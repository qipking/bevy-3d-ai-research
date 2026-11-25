//! 输入补全：`/` 命令、`@` 项目文件，以及插件给的 [`CompletionSource`]。
//!
//! 移植自 rig-tui 0.47 的 `complete.rs`（681 行）。数据模型
//! （[`CompletionSource`] / [`CompletionTrigger`] / [`Candidate`] /
//! [`Completion`] / [`CompletionKind`] / `SHOWN`）、匹配与排序
//! （[`match_at`] / [`token`] / [`path_items`]）、触发器汇总（[`markers`]）
//! 的名字与规则都按上游来；业务方可以照上游文档写代码。
//!
//! 一帧里的顺序和上游一样：
//!
//! ```text
//! PreUpdate   receive_paths   收到后台遍历的结果 / 文件变化，必要时重算
//! Prepare     按键            打字、Tab、Esc 改 Editor / CompletionState
//! Layout      follow_input    Editor 变了就算一次（打开/更新/关闭列表）
//! Draw        调用 completion_rows 产行
//! ```
//!
//! `Layout` 在 `Prepare` 之后、`Draw` 之前，所以补全状态总是这一帧输入的结果。
//!
//! # 与上游的差别（这些不是完整实现）
//!
//! - **没有 `TuiView`**：上游把列表存在 `TuiView.completion`；这里用一个独立资源
//!   [`CompletionState`]，字段与生命周期语义（Esc 关掉后 token 不动就不重开）一致。
//!   上游的 `pasted` / `refused` 抑制（粘贴后、被拒命令放回输入后不弹列表）没做：
//!   我们的粘贴在 `keys.rs`、没有这两条信号；`InputContext` 也没移植，
//!   [`typed`] 是它的等价物。
//! - **`@` 的文件清单不是 `rig_tools::workspace::Workspace`**：这个 crate 是纯前端、
//!   不依赖 rig-tools，所以用 `std::fs` 自己走（[`walk`]）。它只跳过 `.git` /
//!   `target` / `node_modules` 和所有以 `.` 开头的名字，**不读 `.gitignore` /
//!   `.ignore` / 全局 git excludes**（上游 `Listing::Visible` 用 ignore crate，
//!   这三样都尊重）。空目录也不列（和上游一样：目录来自文件路径的祖先）。
//! - **后台遍历的写法**：仍然是 `IoTaskPool` + [`Running`] + `FileWatch` 防抖
//!   （和上游同一套），只是没有 `rig_tools::blocking`，同步的 `std::fs` 直接在
//!   IO 池的 async 任务里跑，会占住一个 IO 线程。
//! - **`@` 引用的解析**：上游用 `rig_tools::attach::Reference`（带空格的
//!   `@"my notes.md"`）；这里有一份语义相同的本地实现
//!   （[`reference_typing`] / [`reference_format`]），规则逐条照抄，只是位置在
//!   本模块里。
//! - **列表落点**：上游 `render::draw_completion` 画 8 行浮层；这里按用户要求
//!   做成输入框上方的 tile —— 分裂树里右列的一个叶子（[`PanelId::Complete`]），
//!   关着时 `Fixed(0)`，打开时 `Fixed(6 行)`，由 [`sync_completion_tile`] 改树、
//!   [`draw_completion`] 产行。候选 + 官方提示行都在这个 tile 里，不是浮层。

use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::system::SystemParam;
use bevy::log::warn;
use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, TaskPool};
use rig_ecs::tasks::{Running, Wake};
use rig_harness::commands::SlashCommand;
use rig_harness::main_agent::ShownAgent;
use rig_harness::services::{ProjectRoot, WorkDir};
use rig_harness::watch::{FileChanges, FileWatch};
use rig_tui::ratatui::text::{Line, Span};

use crate::editor::Editor;
use crate::frame::{FrameTree, PanelAreas};
use crate::panels::{KeyedRow, PanelId, PanelRows, PanelRoots};
use crate::theme::Theme;
use crate::transcript::Row;

/// 候选列表最多列几项（上游 `SHOWN`，只用在 `@` 的路径上）。
pub const SHOWN: usize = 50;

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

/// 插件加进补全的候选，挂在自己的实体上。
///
/// 两种触发（和上游一样）：
/// - [`CompletionSource::marker`]：输入里任意位置一个以 `#` 这类标记开头的词，
///   例如 issue 号；
/// - [`CompletionSource::arg`]：某个 `/` 命令的参数，例如 `/model` 的模型名。
///
/// `/` 与 `@` 是视图自己的（命令名与项目路径），插件给这两个标记的 source
/// 什么也不补。同一个触发可以有几个 source，候选相加；替换时再 insert 一次
/// 就是"这个触发现在只有这些候选"。
///
/// ```text
/// let branches = CompletionSource::arg("checkout").candidate("main", "").candidate("dev", "2 ahead");
/// let issues = CompletionSource::marker('#').candidate("142", "Crash on empty input");
/// commands.spawn_batch([branches, issues]);
/// ```
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct CompletionSource {
    /// 补什么。
    pub trigger: CompletionTrigger,
    /// 候选。同样匹配时按这里的先后列。
    pub candidates: Vec<Candidate>,
}

/// 一个 [`CompletionSource`] 补什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionTrigger {
    /// 输入里任意位置一个以这个标记开头的词，例如 `#`。`/` 与 `@` 是视图
    /// 自己的（命令与路径）：给这两个标记的 source 什么也不补。
    Marker(char),
    /// `/` 命令 `command`（带不带 `/` 都行）的第 `position` 个参数（从 0 起）。
    Arg {
        /// 命令名。
        command: String,
        /// 第几个参数，从 0 起是命令名后的第一个词。
        position: usize,
    },
}

impl CompletionSource {
    /// 还没有候选，补以 `marker` 开头的词。
    pub fn marker(marker: char) -> Self {
        Self::new(CompletionTrigger::Marker(marker))
    }

    /// 还没有候选，补 `command` 的第一个参数。
    pub fn arg(command: impl Into<String>) -> Self {
        Self::new(CompletionTrigger::Arg {
            command: command.into(),
            position: 0,
        })
    }

    fn new(trigger: CompletionTrigger) -> Self {
        Self {
            trigger,
            candidates: Vec::new(),
        }
    }

    /// 同一个 source，改补第 `position` 个参数（从 0 起）；标记 source 不变。
    pub fn at(mut self, at: usize) -> Self {
        if let CompletionTrigger::Arg { position, .. } = &mut self.trigger {
            *position = at;
        }
        self
    }

    /// 同一个 source，再多一个候选 `text`，`detail` 是说明（可以是空的）。
    pub fn candidate(mut self, text: impl Into<String>, detail: impl Into<String>) -> Self {
        self.candidates.push(Candidate {
            text: text.into(),
            detail: detail.into(),
        });
        self
    }

    /// 它是否补 `kind` 这种 token（`command` 不含 `/`）。
    fn is_for(&self, kind: CompletionKind, command: &str) -> bool {
        match (&self.trigger, kind) {
            (CompletionTrigger::Marker(marker), CompletionKind::Marker(typed)) => *marker == typed,
            (
                CompletionTrigger::Arg {
                    command: own,
                    position,
                },
                CompletionKind::Arg(typed),
            ) => own.trim_start_matches('/') == command && *position == typed,
            _ => false,
        }
    }

    /// 它给 [`token`] 加的那个标记（`/`、`@` 不算），没有就是 `None`。
    fn marker_char(&self) -> Option<char> {
        match self.trigger {
            CompletionTrigger::Marker(marker) if !VIEW_MARKERS.contains(&marker) => Some(marker),
            _ => None,
        }
    }
}

/// 视图自己补的两个标记：命令与路径。
const VIEW_MARKERS: [char; 2] = ['/', '@'];

/// 一个 [`Completion`] 补什么。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletionKind {
    /// 开头一个 `/` 后面的命令名。
    Command,
    /// 开头 `/name` 的命令的第几个参数（从 0 起）。
    Arg(usize),
    /// `@` 后面的路径。
    Path,
    /// [`CompletionSource`] 标记后面的词。
    Marker(char),
}

impl CompletionKind {
    /// token 以什么开头；参数什么也不以。
    pub fn marker(self) -> Option<char> {
        match self {
            Self::Command => Some('/'),
            Self::Arg(_) => None,
            Self::Path => Some('@'),
            Self::Marker(marker) => Some(marker),
        }
    }
}

/// 一个候选。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candidate {
    /// 显示并插入的文本，不含标记（`/`、`@`、`#`）。
    pub text: String,
    /// 跟在后面的说明。
    pub detail: String,
}

/// 打开的候选列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// 补什么。
    pub kind: CompletionKind,
    /// 被补的 token 在输入里的字节偏移：标记的位置，或参数第一个字符。
    pub start: usize,
    /// 标记后面打出来的文本，或参数打出来的文本。
    pub query: String,
    /// 列出来的候选，最好的在前。
    pub items: Vec<Candidate>,
    /// Tab / Enter 会放进输入的是 `items` 里的第几个。
    pub selected: usize,
    /// 候选是否来自一趟已经走完的文件遍历（`@` 用）。
    complete: bool,
    /// 这个词是否是一个还没闭合的引号路径（Enter 先补完、闭合引号再发送）。
    open: bool,
}

impl Completion {
    /// 接受选中项时替换 token 的文本：标记 + 候选，目录不带空格（等用户接着
    /// 打下面的路径）。路径按 [`reference_format`] 写：要引号的加引号，目录
    /// 的左引号留着不闭合。
    pub fn chosen(&self) -> Option<String> {
        let item = self.items.get(self.selected)?;
        if self.kind == CompletionKind::Path {
            let reference = reference_format(&item.text);
            return Some(match item.text.ends_with('/') {
                true => reference.strip_suffix('"').unwrap_or(&reference).to_owned(),
                false => format!("{reference} "),
            });
        }
        let space = if item.text.ends_with('/') { "" } else { " " };
        let marker = self.kind.marker().map(String::from).unwrap_or_default();
        Some(format!("{marker}{}{space}", item.text))
    }

    /// token 是否已经正好是选中项，Enter 该发送而不是补全：路径按
    /// [`reference_format`] 写出来比较。
    pub fn is_exact(&self) -> bool {
        let Some(item) = self.items.get(self.selected) else {
            return false;
        };
        match self.kind {
            CompletionKind::Path => {
                !self.open && reference_format(&item.text) == format!("@{}", self.query)
            }
            _ => item.text == self.query,
        }
    }

    /// 选中项上/下移 `step` 位，停在两端。
    pub fn select(&mut self, step: isize) {
        let count = self.items.len();
        if count > 0 {
            self.selected = self.selected.saturating_add_signed(step).min(count - 1);
        }
    }

    /// 把选中项放进 `editor`，替换从 [`Self::start`] 到光标的那段。
    /// 返回是否换了；不是完整 token 的 `start` 一律不换。
    pub fn accept(&self, editor: &mut Editor) -> bool {
        let Some(text) = self.chosen() else {
            return false;
        };
        let start = self.start;
        let cursor = editor.cursor();
        if start > cursor
            || !editor.text.is_char_boundary(start)
            || !editor.text.is_char_boundary(cursor)
        {
            return false;
        }
        editor.text.replace_range(start..cursor, &text);
        editor.set_cursor(start + text.len());
        true
    }

    /// 候选是否来自一趟走完的遍历（`@` 用）。
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// 这个词是否是一个还没闭合的引号路径。
    pub fn open(&self) -> bool {
        self.open
    }

    /// 用给定候选造一个列表。
    ///
    /// 插件手工推一批候选时用它（上游靠 [`CompletionSource`] 走查询；这里也给一条直路）。
    /// `complete`/`open` 两个内部标记都取 `false`：这不是一趟后台遍历的结果。
    pub fn from_items(
        kind: CompletionKind,
        start: usize,
        query: impl Into<String>,
        items: Vec<Candidate>,
    ) -> Self {
        Self {
            kind,
            start,
            query: query.into(),
            items,
            selected: 0,
            complete: false,
            open: false,
        }
    }
}

/// 打开着的补全（上游 `TuiView.completion` + `TuiView.dismissed`）。
///
/// `keys.rs` 读它做 Tab / ↑↓ / Esc；[`follow_input`] 每帧输入变了就更新它。
#[derive(Resource, Default)]
pub struct CompletionState {
    /// 打开着的列表；`None` 表示没开。
    completion: Option<Completion>,
    /// Esc 关掉时那个 token 的起点；token 不动就不再重开（上游 `dismissed`）。
    dismissed: Option<usize>,
}

impl CompletionState {
    /// 打开着的列表。
    pub fn completion(&self) -> Option<&Completion> {
        self.completion.as_ref()
    }

    /// 打开着的列表（可改，例如 ↑↓ 改选中项）。
    pub fn completion_mut(&mut self) -> Option<&mut Completion> {
        self.completion.as_mut()
    }

    /// 是否开着。
    pub fn is_open(&self) -> bool {
        self.completion.is_some()
    }

    /// 直接摆一个列表（插件/测试用；正常路径是 [`follow_input`]）。
    pub fn show(&mut self, completion: Completion) {
        self.completion = Some(completion);
        self.dismissed = None;
    }

    /// 取走列表（Tab / Enter 接受时用）。
    pub fn take(&mut self) -> Option<Completion> {
        self.completion.take()
    }

    /// Esc：关掉列表，并记住这个 token 的起点，直到 token 变了才允许重开。
    pub fn dismiss(&mut self) {
        if let Some(completion) = self.completion.take() {
            self.dismissed = Some(completion.start);
        }
    }

    /// 直接关掉（例如粘贴后），下次输入变了就会重新算。
    pub fn close(&mut self) {
        self.completion = None;
        self.dismissed = None;
    }
}

// ---------------------------------------------------------------------------
// `@` 引用的本地实现（上游 rig_tools::attach::Reference）
// ---------------------------------------------------------------------------

/// [`reference_format`] 会加引号的结尾标点（上游 `attach::TRAILING`）。
const TRAILING: [char; 10] = [',', '.', ';', ':', '!', '?', ')', '"', '\'', '`'];

/// 正在打的 `@` 引用。
struct TypingReference<'a> {
    /// `@` 的字节偏移。
    start: usize,
    /// `@` 后面的路径。
    path: Cow<'a, str>,
    /// 是否在引号里（还没闭合）。
    quoted: bool,
}

/// `text` 末尾正在打的 `@` 引用，语义同上游 `Reference::typing`：
/// 引号里未闭合的一直算；引号外的只在最后一个词就是它时算（路径到下一个
/// 空白为止，标点留着）。
fn reference_typing(text: &str) -> Option<TypingReference<'_>> {
    let mut from = 0;
    loop {
        let at = next_reference_start(text, from)?;
        let after = text.get(at + 1..)?;
        if let Some(rest) = after.strip_prefix('"') {
            match closed_quote(rest) {
                Some((_, len)) => from = at + 2 + len,
                None => {
                    return Some(TypingReference {
                        start: at,
                        path: unclosed_quote(rest),
                        quoted: true,
                    });
                }
            }
            continue;
        }
        let token = after.split(char::is_whitespace).next().unwrap_or_default();
        from = at + 1 + token.len();
        if from == text.len() {
            return Some(TypingReference {
                start: at,
                path: Cow::Borrowed(token),
                quoted: false,
            });
        }
    }
}

/// `text` 从字节 `from` 起下一个"词首"的 `@`（前面是空白或什么都没有）。
fn next_reference_start(text: &str, from: usize) -> Option<usize> {
    text.get(from..)?
        .match_indices('@')
        .map(|(at, _)| from + at)
        .find(|&at| {
            text.get(..at)
                .and_then(|before| before.chars().next_back())
                .is_none_or(char::is_whitespace)
        })
}

/// `rest`（开引号之后）里闭合的引号路径：路径（`""` 读成一个 `"`）与含
/// 闭引号的字节长度；没有闭引号时 `None`。
fn closed_quote(rest: &str) -> Option<(Cow<'_, str>, usize)> {
    let mut at = 0;
    loop {
        let close = at + rest.get(at..)?.find('"')?;
        if rest.get(close + 1..)?.starts_with('"') {
            at = close + 2;
            continue;
        }
        let inside = rest.get(..close)?;
        let path = match inside.contains("\"\"") {
            true => Cow::Owned(inside.replace("\"\"", "\"")),
            false => Cow::Borrowed(inside),
        };
        return Some((path, close + 1));
    }
}

/// 没有闭合的引号路径：`rest` 全部，`""` 读成一个 `"`。
fn unclosed_quote(rest: &str) -> Cow<'_, str> {
    Cow::Owned(rest.replace("\"\"", "\""))
}

/// `path` 写成 `@` 引用，语义同上游 `Reference::format`：空路径、含空白、
/// 以 `"` 开头、以标点结尾、看起来像行号范围（`notes.md:2`、`a:10-40`）
/// 的加引号，引号里的 `"` 写成 `""`。
fn reference_format(path: &str) -> String {
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

// ---------------------------------------------------------------------------
// 后台文件索引（`@` 的路径）
// ---------------------------------------------------------------------------

/// 项目里的路径，给 `@` 补全用（上游 `FileIndex`）。
///
/// 第一趟走盘在后台（`IoTaskPool` + [`Running`]）跑，结果作为 [`WalkedPaths`]
/// 落到遍历实体上，[`receive_paths`] 收进这里；根目录有一个 [`FileWatch`]，
/// 它报来文件变化时把 `fresh` 清掉，下次 `@` 的 token 出现就再走一趟。
#[derive(Resource, Default)]
pub struct FileIndex {
    /// 文件与目录（目录以 `/` 结尾），相对根目录。
    paths: Arc<Vec<String>>,
    /// 是否有一趟遍历在跑（结束成为 [`WalkedPaths`]）。
    walking: bool,
    /// 路径是否是这个项目最新的样子（watcher 上次看到的）。
    fresh: bool,
    /// 第一次 `@` 之后根目录的 watch。
    watch: Option<Entity>,
    /// 路径相对的根。
    root: Option<PathBuf>,
}

/// 一趟遍历穿过的根与找到的路径（失败时 `None`），挂在遍历实体上。
#[derive(Component)]
pub(crate) struct WalkedPaths(PathBuf, Option<Arc<Vec<String>>>);

impl From<(PathBuf, Option<Vec<String>>)> for WalkedPaths {
    fn from((root, paths): (PathBuf, Option<Vec<String>>)) -> Self {
        Self(root, paths.map(Arc::new))
    }
}

impl FileIndex {
    /// 现在列得出的路径（目录以 `/` 结尾）。
    pub fn paths(&self) -> &[String] {
        self.paths.as_slice()
    }

    /// 是否有一趟遍历在跑。
    pub fn is_walking(&self) -> bool {
        self.walking
    }

    /// 路径是否是项目最新的样子。
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// 路径相对的根（还没见过 `@` 时 `None`）。
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 根目录的 [`FileWatch`] 实体（还没见过 `@` 时 `None`）。
    pub fn watch(&self) -> Option<Entity> {
        self.watch
    }

    /// 看着 `root`，然后在 `root` 变了、或者没有遍历在跑且路径不新鲜时，
    /// 在后台开始一趟遍历（上游 `FileIndex::refresh`）。
    fn refresh(&mut self, wake: &Wake, root: PathBuf, commands: &mut Commands) {
        if self.root.as_ref() != Some(&root) {
            if let Some(watch) = self.watch.take() {
                commands.entity(watch).despawn();
            }
            (self.paths, self.fresh) = (Arc::default(), false);
            self.root = Some(root.clone());
        }
        if self.watch.is_none() {
            let watch = (
                Name::new("rig_niri: @ 路径"),
                FileWatch::tree(root.clone()),
            );
            self.watch = Some(commands.spawn(watch).id());
        }
        if self.walking || self.fresh {
            return;
        }
        let walked = root.clone();
        let directory = root.clone();
        // 没有 rig_tools::blocking：同步的 std::fs 直接在这个 IO 池的
        // async 任务里跑（会占住一个 IO 线程，见模块文档）。
        let task = async move {
            let paths = match walk(&directory) {
                Ok(paths) => Some(paths),
                Err(failure) => {
                    warn!("列不出项目的文件：{failure}");
                    None
                }
            };
            (walked, paths)
        };
        let pool = IoTaskPool::get_or_init(TaskPool::default);
        let walking = Running::spawn_into::<WalkedPaths, _>(pool, wake, task);
        commands.spawn((Name::new("rig_niri: @ 路径遍历"), walking));
        // 从这以后看到的变化都让这趟遍历过期。
        (self.walking, self.fresh) = (true, true);
    }
}

/// 收 `root` 下的文件与装着它们的目录（相对 `root`，目录以 `/` 结尾），
/// 跳过 `.git`、`target`、`node_modules` 和所有以 `.` 开头的名字。
///
/// 这是上游 `walk` 的**简化**：上游用 `rig_tools::workspace::Workspace`
/// （ignore crate，尊重 `.gitignore` / `.ignore` / git excludes 与隐藏名，
/// 还是 `Listing::Visible` 的规则）；这里只有常见目录名的黑名单。
/// 根读不了返回 `Err`（上游也一样：只有根这一层的失败算失败），子目录读不了
/// 跳过。空目录不列（和上游一样：目录来自文件路径的祖先）。
fn walk(root: &Path) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    let mut paths = Vec::new();
    let mut directories = HashSet::new();
    for file in &files {
        let mut above: Vec<&Path> = file.ancestors().skip(1).collect();
        above.retain(|directory| !directory.as_os_str().is_empty());
        for directory in above.into_iter().rev() {
            if directories.insert(directory) {
                paths.push(format!("{}/", path_text(directory)));
            }
        }
        paths.push(path_text(file));
    }
    Ok(paths)
}

/// 收集 `directory` 下（相对 `root`）的文件，按名字排序深度优先。
fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // 隐藏名（含 `.git`）、`target` 与 `node_modules` 不进索引。
        if name.starts_with('.') || matches!(&*name, "target" | "node_modules") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        entries.push((entry.path(), file_type.is_dir()));
    }
    entries.sort();
    for (path, is_dir) in entries {
        if is_dir {
            // 子目录读不了（权限之类）就跳过它的内容；上游也只在根上算失败。
            let _ = collect_files(root, &path, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// 路径写成 `@` 补全的样子：`/` 分隔；第一个名字是 `~` 或带 `:` 的加
/// `./` 前缀，免得被读成家目录或 `HOST:PATH`（上游 `walk` 里的 `text`）。
fn path_text(path: &Path) -> String {
    let text = match cfg!(windows) {
        true => path.to_string_lossy().replace('\\', "/"),
        false => path.to_string_lossy().into_owned(),
    };
    let first = text.split('/').next().unwrap_or_default();
    match first == "~" || first.contains(':') {
        true => format!("./{text}"),
        false => text,
    }
}

/// 项目根：显示的 agent 的 [`WorkDir`]（它的引用从哪里解析），否则
/// [`ProjectRoot`]，否则当前目录（上游 `Sources::project` 的前半，后半的
/// `ProjectWorkspace` 在这里没有——路径用本机的 `std::fs` 走）。
fn project_root(
    shown: Option<&ShownAgent>,
    dirs: &Query<&WorkDir>,
    root: Option<&ProjectRoot>,
) -> PathBuf {
    let directory = shown.and_then(|shown| shown.0).and_then(|agent| dirs.get(agent).ok());
    match (directory, root) {
        (Some(directory), _) => directory.0.clone(),
        (None, Some(root)) => root.0.clone(),
        (None, None) => PathBuf::from("."),
    }
}

// ---------------------------------------------------------------------------
// 匹配：`match_at` / `token` / `path_items`
// ---------------------------------------------------------------------------

/// `query` 第一次出现在 `text` 里的字节位置，忽略大小写；没有就是 `None`。
fn match_at(query: &str, text: &str) -> Option<usize> {
    text.to_lowercase().find(&query.to_lowercase())
}

/// 结束在光标处的 `/命令`、参数、`@路径` 或标记词（`markers` 是插件加的
/// 标记）；参数带命令名与位置。返回 `(类型, token 起点字节, 已打的文本, 命令名)`。
fn token<'a>(
    text: &'a str,
    cursor: usize,
    markers: &[char],
) -> Option<(CompletionKind, usize, Cow<'a, str>, &'a str)> {
    let before = text.get(..cursor)?;
    if let Some(reference) = reference_typing(before) {
        return Some((CompletionKind::Path, reference.start, reference.path, ""));
    }
    let start = before
        .char_indices()
        .rfind(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let word = before.get(start..)?;
    if let Some(query) = word.strip_prefix('/')
        && start == 0
    {
        return Some((CompletionKind::Command, start, query.into(), ""));
    }
    if let Some(marker) = word.chars().next().filter(|first| markers.contains(first)) {
        let query = word.get(marker.len_utf8()..)?;
        return Some((CompletionKind::Marker(marker), start, query.into(), ""));
    }
    let command = text.strip_prefix('/')?.split_whitespace().next()?;
    let position = before
        .get(..start)?
        .split_whitespace()
        .count()
        .checked_sub(1)?;
    Some((CompletionKind::Arg(position), start, word.into(), command))
}

/// `query` 最好的 [`SHOWN`] 个路径：文件名匹配的排在只有目录匹配的前面，
/// 然后按匹配位置，浅的路径在前（上游 `path_items`）。
fn path_items(paths: &[String], query: &str) -> Vec<Candidate> {
    let mut ranked: Vec<((bool, usize, usize), &String)> = paths
        .iter()
        .filter_map(|path| {
            let trimmed = path.trim_end_matches('/');
            let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let (in_directory, at) = match match_at(query, name) {
                Some(at) => (false, at),
                None => (true, match_at(query, path)?),
            };
            Some(((in_directory, at, trimmed.matches('/').count()), path))
        })
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .take(SHOWN)
        .map(|(_, path)| Candidate {
            text: path.clone(),
            detail: String::new(),
        })
        .collect()
}

/// `/` 命令的候选：`query` 匹配命令名（忽略了 `/`），按匹配位置、再按名字。
fn command_items<'a>(
    query: &str,
    commands: impl Iterator<Item = (&'a str, &'a str)>,
) -> Vec<Candidate> {
    let mut ranked: Vec<(usize, &str, &str)> = commands
        .filter_map(|(name, help)| Some((match_at(query, name)?, name, help)))
        .collect();
    ranked.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    ranked
        .into_iter()
        .map(|(_, name, help)| Candidate {
            text: name.to_owned(),
            detail: help.to_owned(),
        })
        .collect()
}

/// 插件给的候选：`query` 匹配候选文本，按匹配位置，同样匹配的保持 source
/// 里的先后（上游 `update` 的 `Arg` / `Marker` 分支）。
fn candidates_items<'a>(
    query: &str,
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Vec<Candidate> {
    let mut ranked: Vec<(usize, &Candidate)> = candidates
        .filter_map(|item| Some((match_at(query, &item.text)?, item)))
        .collect();
    ranked.sort_by_key(|(at, _)| *at);
    ranked.into_iter().map(|(_, item)| item.clone()).collect()
}

/// 所有 source 加的标记，排序去重（上游 `markers`）。
fn markers<'a>(sources: impl IntoIterator<Item = &'a CompletionSource>) -> Vec<char> {
    let mut markers: Vec<char> = sources
        .into_iter()
        .filter_map(CompletionSource::marker_char)
        .collect();
    markers.sort_unstable();
    markers.dedup();
    markers
}

// ---------------------------------------------------------------------------
// 触发源与每帧的更新
// ---------------------------------------------------------------------------

/// 补全列什么：`/` 命令与插件加的 [`CompletionSource`]（上游 `Sources`）。
#[derive(SystemParam)]
pub(crate) struct Sources<'w, 's> {
    /// 注册的 `/` 命令（名字含前导 `/`，被 `Disabled` 的不算）。
    commands: Query<'w, 's, (&'static Name, &'static SlashCommand, Option<&'static Disabled>)>,
    /// 插件加的候选。
    added: Query<'w, 's, &'static CompletionSource>,
    /// 项目根。
    root: Option<Res<'w, ProjectRoot>>,
    /// 现在显示的 agent。
    shown: Option<Res<'w, ShownAgent>>,
    /// 各 agent 的工作目录。
    dirs: Query<'w, 's, &'static WorkDir>,
}

impl Sources<'_, '_> {
    /// `@` 路径相对哪个目录。
    fn project(&self) -> PathBuf {
        project_root(self.shown.as_deref(), &self.dirs, self.root.as_deref())
    }
}

/// 光标处的 token 与标记后面打出来的文本（上游 `typed`，即
/// `InputContext::typed` 的等价物）。
pub fn typed(input: &Editor, sources: &Query<&CompletionSource>) -> Option<(CompletionKind, String)> {
    let (kind, _, query, _) = token(input.text(), input.cursor(), &markers(sources.iter()))?;
    Some((kind, query.into_owned()))
}

/// 当前 token 的候选，最好的在前（上游 `update` 里按 `kind` 分派的三个分支）。
fn items_for(
    kind: CompletionKind,
    query: &str,
    command: &str,
    sources: &Sources,
    index: &FileIndex,
) -> Vec<Candidate> {
    match kind {
        CompletionKind::Command => command_items(
            query,
            sources.commands.iter().filter_map(|(name, command, disabled)| {
                disabled.is_none().then(|| {
                    (
                        name.as_str().trim_start_matches('/'),
                        command.help.as_str(),
                    )
                })
            }),
        ),
        CompletionKind::Arg(_) | CompletionKind::Marker(_) => {
            let candidates = sources
                .added
                .iter()
                .filter(|source| source.is_for(kind, command))
                .flat_map(|source| &source.candidates);
            candidates_items(query, candidates)
        }
        CompletionKind::Path => path_items(index.paths.as_slice(), query),
    }
}

/// 按输入打开、更新或关掉补全（上游 `update`）。
///
/// - 正原样显示一条历史提示词时不补（Up/Down 走过历史时不弹）；
/// - 同一个 token 已经开着、内容也一样、source 没重新插入（`renewed`）就
///   不重算；
/// - `@` 的 token 会顺手让 [`FileIndex`] 在后台走一趟；
/// - 列出同样的候选时保留选中的那一项，它还在的话。
fn update(
    state: &mut CompletionState,
    input: &Editor,
    index: &mut FileIndex,
    sources: &Sources,
    renewed: bool,
    wake: &Wake,
    commands: &mut Commands,
) {
    if input.recalled() {
        state.completion = None;
        state.dismissed = None;
        return;
    }
    let found = token(input.text(), input.cursor(), &markers(sources.added.iter()));
    let Some((kind, start, query, command)) = found else {
        state.completion = None;
        state.dismissed = None;
        return;
    };
    let query = &*query;
    let quoted = input
        .text()
        .get(..input.cursor())
        .and_then(reference_typing)
        .is_some_and(|reference| reference.quoted);
    if state.dismissed == Some(start) {
        state.completion = None;
        return;
    }
    state.dismissed = None;
    if kind == CompletionKind::Path {
        index.refresh(wake, sources.project(), commands);
    }
    let complete = !index.walking;
    if let Some(open) = &state.completion
        && open.kind == kind
        && open.start == start
        && open.query == query
        && (kind != CompletionKind::Path || open.complete == complete)
        && !renewed
    {
        return;
    }
    let items = items_for(kind, query, command, sources, index);
    let query = query.to_owned();
    // 列表重列时，原来选中的那一项还在就仍然选它（上游的 `selected`）。
    let selected = state
        .completion
        .as_ref()
        .filter(|open| open.kind == kind && open.start == start)
        .and_then(|open| open.items.get(open.selected))
        .and_then(|was| items.iter().position(|item| item == was))
        .unwrap_or(0);
    let completion = (!items.is_empty()).then_some(Completion {
        kind,
        start,
        query,
        items,
        selected,
        complete,
        open: quoted,
    });
    // 相同就不写，免得每帧都触发 Res 的变化检测。
    if state.completion != completion {
        state.completion = completion;
    }
}

/// 输入变了、或者插件的 source 变了，就更新补全（上游 `follow_input`）。
pub(crate) fn follow_input(
    mut index: ResMut<FileIndex>,
    mut state: ResMut<CompletionState>,
    input: Res<Editor>,
    sources: Sources,
    (changed, mut removed): (
        Query<(), Changed<CompletionSource>>,
        RemovedComponents<CompletionSource>,
    ),
    (wake, mut commands): (Option<Res<Wake>>, Commands),
) {
    let renewed = !changed.is_empty() || removed.read().count() > 0;
    if input.is_changed() || renewed {
        // harness 装了内核就有真的 `Wake`；测试里没有就用默认的（什么也不做）。
        let wake = wake.as_deref().cloned().unwrap_or_default();
        update(
            &mut state,
            &input,
            &mut index,
            &sources,
            renewed,
            &wake,
            &mut commands,
        );
    }
}

/// 文件 watch 变了就让索引过期；收下走完的遍历；这两种情况都重算一次补全，
/// 好让开着的 `@` 列表跟上（上游 `receive_paths`）。
pub(crate) fn receive_paths(
    mut index: ResMut<FileIndex>,
    mut changes: MessageReader<FileChanges>,
    walked: Option<Single<(Entity, &WalkedPaths)>>,
    mut state: ResMut<CompletionState>,
    input: Res<Editor>,
    sources: Sources,
    (wake, mut commands): (Option<Res<Wake>>, Commands),
) {
    let watch = index.watch;
    let changed = (changes.read())
        .filter(|change| Some(change.watch) == watch && change.event.names_files())
        .count()
        > 0;
    if changed {
        index.fresh = false;
    }
    let received = walked.map(|walked| walked.into_inner());
    let mut walked = false;
    if let Some((walk, WalkedPaths(root, paths))) = received {
        // 一个已经离开的根的遍历是过期的；失败的遍历保留旧路径，下次变化
        // 或者按键再试（上游同样的处理）。
        match paths {
            Some(paths) if index.root.as_ref() == Some(root) => {
                index.paths = paths.clone();
                walked = true;
            }
            Some(_) => {}
            None => index.fresh = false,
        }
        index.walking = false;
        commands.entity(walk).despawn();
    }
    if changed || walked {
        let wake = wake.as_deref().cloned().unwrap_or_default();
        update(
            &mut state,
            &input,
            &mut index,
            &sources,
            false,
            &wake,
            &mut commands,
        );
    }
}

/// 补全的插件：资源、文件索引的收尾（`PreUpdate`）、输入后的更新（`Layout`），
/// 以及输入框上方补全 tile 的开合（`Layout`）与产行（`Draw`）。
///
/// 接线由 [`FeathersViewPlugin`](crate::FeathersViewPlugin) 注册；按键（Tab /
/// ↑↓ / Esc）在 `editor.rs` 的 `completion_act` 里接到 [`CompletionState`]。
pub struct CompletionPlugin;

impl Plugin for CompletionPlugin {
    fn build(&self, app: &mut App) {
        // 没有 harness 的应用（测试）也要有文件变化的消息；已经有了就不
        // 重复装 WatchPlugin（会多一个写消息的 observer）。
        if !app.world().contains_resource::<Messages<FileChanges>>() {
            app.add_plugins(rig_harness::watch::WatchPlugin);
        }
        app.init_resource::<CompletionState>()
            .init_resource::<FileIndex>()
            .add_systems(PreUpdate, receive_paths)
            .add_systems(
                Update,
                follow_input.in_set(crate::FeathersSystems::Layout),
            )
            // 补全状态这一帧先更新（`follow_input`），再按开合改树的高度；
            // 排 `lay_out_frame` 之前，补全打开的同一帧输入就让出 6 行。
            .add_systems(
                Update,
                sync_completion_tile
                    .after(follow_input)
                    .before(crate::frame::lay_out_frame)
                    .in_set(crate::FeathersSystems::Layout),
            )
            .add_systems(
                Update,
                draw_completion.in_set(crate::FeathersSystems::Draw),
            );
    }
}

// ---------------------------------------------------------------------------
// 产行
// ---------------------------------------------------------------------------

/// 把补全列表变成行：每项一行，选中项用 `> ` 打头、强调色，说明用 dim，
/// 宽度按上游 [`rig_tui::wrap::wrap_all`] 折（长命令名/路径不越界）。
///
/// 上游 `render::draw_completion` 是 8 行浮层；我们的行是值，高度由
/// [`completion_tile_rows`] 按补全 tile 的可见行数截断（`@` 的路径已经在
/// [`path_items`] 里截到 [`SHOWN`]）。
pub fn completion_rows(completion: &Completion, width: usize, theme: &Theme) -> Vec<Row> {
    let width = width.max(1);
    let marker = completion.kind.marker().map(String::from).unwrap_or_default();
    let mut rows = Vec::new();
    for (index, item) in completion.items.iter().enumerate() {
        let selected = index == completion.selected;
        let (prefix, style) = match selected {
            true => ("> ", theme.accent),
            false => ("  ", theme.fg),
        };
        let mut spans = vec![
            Span::styled(prefix.to_owned(), style),
            Span::styled(format!("{marker}{}", item.text), style),
        ];
        if !item.detail.is_empty() {
            spans.push(Span::styled(format!("  {}", item.detail), theme.dim));
        }
        for line in rig_tui::wrap::wrap_all(&[Line::from(spans)], width) {
            rows.push(Row::new(line));
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// 输入框上方的补全 tile
// ---------------------------------------------------------------------------

/// 补全打开时 tile 占几行。
pub const TILE_ROWS: f32 = 6.0;

/// 官方补全提示行，逐字照抄（`docs/reports/official-tui.md`）。
pub const COMPLETION_HINT: &str = "Tab or Enter completes · Esc closes";

/// 补全 tile 的高度（UI 像素）：开 6 行，关 0。
///
/// 6 行 = `6 * ROW_HEIGHT`（108 px）。tile 是右列的一个固定高度叶子：
/// 打开时输入（比例子）自然让出这 6 行，右列总高不变。
pub fn completion_tile_height(open: bool) -> f32 {
    match open {
        true => TILE_ROWS * crate::transcript::ROW_HEIGHT,
        false => 0.0,
    }
}

/// 补全打开/关闭时改 [`FrameTree`] 里 `Complete` 叶子的高度。
///
/// 关着时保留叶子、只把尺寸设成 `Fixed(0.0)`（不摘除）：右列 gap 是
/// [`NO_GAP`](crate::layout::NO_GAP)，0 高度不会多出缝；保留叶子还能让
/// `place_panels` 每帧把它写成 0 高度，重新打开时不用改树的结构。
/// 树里没有 `Complete` 叶子（业务换了整棵树）时什么也不做。
pub fn sync_completion_tile(
    state: Res<CompletionState>,
    tree: Option<ResMut<FrameTree>>,
) {
    let Some(mut tree) = tree else {
        // 只装了 `CompletionPlugin` 的测试 App 没有帧树：没有 tile 可改。
        return;
    };
    let want = crate::layout::Size::Fixed(completion_tile_height(state.is_open()));
    if tree.0.leaf_size(PanelId::Complete) == Some(want) {
        return;
    }
    tree.0.set_leaf_size(PanelId::Complete, want);
}

/// tile 的提示行（官方逐字），按 tile 宽度折行。
pub fn completion_hint_rows(width: usize, theme: &Theme) -> Vec<Row> {
    let width = width.max(1);
    rig_tui::wrap::wrap_all(
        &[Line::from(Span::styled(COMPLETION_HINT, theme.dim))],
        width,
    )
    .into_iter()
    .map(Row::new)
    .collect()
}

/// 补全 tile 这一帧显示的行：候选在前（最多占 `visible_rows - 提示行`），
/// **官方提示行永远在最后**，总行数不超过 `visible_rows`。
pub fn completion_tile_rows(
    completion: Option<&Completion>,
    width: usize,
    visible_rows: usize,
    theme: &Theme,
) -> Vec<Row> {
    let width = width.max(1);
    if visible_rows == 0 {
        return Vec::new();
    }
    let hint = completion_hint_rows(width, theme);
    let mut rows = Vec::new();
    if let Some(completion) = completion {
        let budget = visible_rows.saturating_sub(hint.len());
        rows.extend(completion_rows(completion, width, theme).into_iter().take(budget));
    }
    // 候选少时提示行仍然紧跟在候选下面（官方也是这个顺序）。
    let left = visible_rows.saturating_sub(rows.len());
    rows.extend(hint.into_iter().take(left));
    rows.truncate(visible_rows);
    rows
}

/// 行内容的增量 diff hash（和 `status.rs` 同一做法：样式行 + 右段）。
fn row_hash(row: &Row) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", row.line).hash(&mut hasher);
    if let Some(right) = &row.right {
        format!("{right:?}").hash(&mut hasher);
    }
    hasher.finish()
}

/// 给 tile 的行编上稳定的 key（可见行下标）与内容 hash。
fn keyed_rows(rows: Vec<Row>) -> Vec<KeyedRow> {
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| KeyedRow {
            key: index as u64 + 1,
            hash: row_hash(&row),
            row,
        })
        .collect()
}

/// 每帧把补全状态落到 `Complete` 面板：开着就写候选 + 提示行，关了就清空。
///
/// 宽度/高度取 [`PanelAreas`] 里这块面板的区域（右列宽度，见
/// [`crate::frame::lay_out_frame`]）；面板还不存在（例如 niri 模式）就跳过。
pub fn draw_completion(
    panels: Option<ResMut<PanelRows>>,
    state: Res<CompletionState>,
    areas: Option<Res<PanelAreas>>,
    roots: Option<Res<PanelRoots>>,
    theme: Option<Res<Theme>>,
) {
    let (Some(mut panels), Some(areas), Some(roots)) = (panels, areas, roots) else {
        // 没有面板系统（只装了 `CompletionPlugin` 的测试 App）：只更新状态。
        return;
    };
    let Some(root) = roots.entity(PanelId::Complete) else {
        return;
    };
    let Some(area) = areas.get(root) else {
        return;
    };
    if !state.is_open() {
        panels.set(PanelId::Complete, Vec::new());
        return;
    }
    let theme = theme.map(|theme| *theme.as_ref()).unwrap_or_default();
    let rows = completion_tile_rows(
        state.completion(),
        usize::from(area.cols),
        usize::from(area.rows),
        &theme,
    );
    panels.set(PanelId::Complete, keyed_rows(rows));
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::PanelArea;
    use crate::layout::Size;
    use crate::niri::TileRect;
    use bevy::ecs::system::RunSystemOnce;

    /// 一行纯文本（提示行断言用）。
    fn row_text(row: &Row) -> String {
        row.line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// 一个只有选中项固定的 [`Completion`]。
    fn completion(kind: CompletionKind, text: &str, start: usize, query: &str) -> Completion {
        Completion {
            kind,
            start,
            query: query.to_owned(),
            items: vec![Candidate {
                text: text.to_owned(),
                detail: String::new(),
            }],
            selected: 0,
            complete: true,
            open: false,
        }
    }

    /// 独占的临时目录；测试结束自己删。
    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let at = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rig_niri-complete-{name}-{}-{at}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_leading_slash_token_completes_a_command_name() {
        let (kind, start, query, command) = token("/mo", 3, &[]).unwrap();
        assert_eq!(kind, CompletionKind::Command);
        assert_eq!(start, 0);
        assert_eq!(&*query, "mo");
        assert_eq!(command, "");

        // 不是开头，或者不是词首的 `/` 都不是命令
        assert!(token("see /mo", 7, &[]).is_none());
        // 命令后面的词是参数
        let (kind, start, query, command) = token("/model ac", 9, &[]).unwrap();
        assert_eq!(kind, CompletionKind::Arg(0));
        assert_eq!(start, 7);
        assert_eq!(&*query, "ac");
        assert_eq!(command, "model");
    }

    #[test]
    fn slash_commands_filter_and_rank_by_where_they_match() {
        let commands = [
            ("model", "选择模型"),
            ("mode", ""),
            ("help", "帮助"),
            ("prompt", ""),
        ];
        let items = command_items("mod", commands.iter().copied());
        let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
        // 都在 0 处命中，按名字排
        assert_eq!(texts, ["mode", "model"]);
        assert_eq!(items[1].detail, "选择模型");

        // 只有 "/" 时全列，按名字
        let all = command_items("", commands.iter().copied());
        let texts: Vec<&str> = all.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, ["help", "mode", "model", "prompt"]);

        assert!(command_items("zzz", commands.iter().copied()).is_empty());
    }

    #[test]
    fn a_path_prefix_lists_directories_then_files() {
        let paths: Vec<String> = ["README.md", "src/", "src/lib.rs", "src/main.rs", "docs/"]
            .into_iter()
            .map(String::from)
            .collect();
        let items = path_items(&paths, "src/");
        let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
        // 目录 "src/" 深度 0，文件名不命中、整条路径命中的排后面
        assert_eq!(texts, ["src/", "src/lib.rs", "src/main.rs"]);

        // 文件名命中的排在只有目录命中的前面
        let paths: Vec<String> = ["lib/README.md", "src/lib.rs"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(path_items(&paths, "lib")[0].text, "src/lib.rs");
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(match_at("MODEL", "/model"), Some(1));
        assert_eq!(match_at("readme", "README.md"), Some(0));
        assert_eq!(match_at("docs", "README.md"), None);
        let paths = vec!["ReadMe.md".to_owned()];
        assert_eq!(path_items(&paths, "readme").len(), 1);
    }

    #[test]
    fn an_earlier_match_ranks_higher() {
        let candidates = [
            Candidate {
                text: "amodel".into(),
                detail: String::new(),
            },
            Candidate {
                text: "model".into(),
                detail: String::new(),
            },
            Candidate {
                text: "modelx".into(),
                detail: String::new(),
            },
        ];
        let items = candidates_items("model", candidates.iter());
        let texts: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
        // 0 处命中的两个保持 source 里的先后，1 处命中的最后
        assert_eq!(texts, ["model", "modelx", "amodel"]);
    }

    #[test]
    fn a_source_completes_its_own_trigger_only() {
        let marker = CompletionSource::marker('#').candidate("142", "崩溃");
        assert!(marker.is_for(CompletionKind::Marker('#'), ""));
        assert!(!marker.is_for(CompletionKind::Marker('@'), ""));
        assert_eq!(marker.marker_char(), Some('#'));

        let arg = CompletionSource::arg("/model").at(1).candidate("fast", "");
        assert!(arg.is_for(CompletionKind::Arg(1), "model"));
        assert!(!arg.is_for(CompletionKind::Arg(0), "model"));
        assert!(!arg.is_for(CompletionKind::Arg(1), "pair"));
        assert_eq!(arg.marker_char(), None);

        // `/` 与 `@` 是视图自己的标记
        assert_eq!(CompletionSource::marker('/').marker_char(), None);
        assert_eq!(CompletionSource::marker('@').marker_char(), None);
    }

    #[test]
    fn markers_skip_the_views_own_and_deduplicate() {
        let mut world = World::new();
        world.spawn(CompletionSource::marker('#'));
        world.spawn(CompletionSource::marker('#'));
        world.spawn(CompletionSource::marker('@'));
        world.spawn(CompletionSource::marker('/'));
        world.spawn(CompletionSource::arg("model"));
        let mut sources = world.query::<&CompletionSource>();
        let query = sources.query(&world);
        assert_eq!(markers(query.iter()), vec!['#']);
    }

    #[test]
    fn no_candidates_list_nothing() {
        let paths = vec!["src/lib.rs".to_owned()];
        assert!(path_items(&paths, "zzz").is_empty());
        let empty: Vec<Candidate> = Vec::new();
        assert!(candidates_items("x", empty.iter()).is_empty());
        assert!(command_items("zzz", [("model", "")].into_iter()).is_empty());
    }

    #[test]
    fn path_items_shows_at_most_shown() {
        let paths: Vec<String> = (0..SHOWN + 10)
            .map(|index| format!("file{index:03}.rs"))
            .collect();
        let items = path_items(&paths, "file");
        assert_eq!(items.len(), SHOWN);
        assert_eq!(items[0].text, "file000.rs");
        assert_eq!(items[SHOWN - 1].text, format!("file{:03}.rs", SHOWN - 1));
    }

    #[test]
    fn token_offsets_are_bytes_through_cjk_and_quotes() {
        // `@` 路径：起点是 `@` 的字节偏移（"你好 " = 7 字节），路径按字节切
        let text = "你好 @src/文件.rs";
        let (kind, start, query, _) = token(text, text.len(), &[]).unwrap();
        assert_eq!(kind, CompletionKind::Path);
        assert_eq!(start, 7);
        assert_eq!(&*query, "src/文件.rs");

        // 标记词：`#` 的偏移（"修复 " = 7 字节）与后面的查询
        let text = "修复 #142";
        let (kind, start, query, _) = token(text, text.len(), &['#']).unwrap();
        assert_eq!(kind, CompletionKind::Marker('#'));
        assert_eq!(start, 7);
        assert_eq!(&*query, "142");

        // 命令参数：`/model 你好` 的参数从字节 7 起
        let text = "/model 你好";
        let (kind, start, query, command) = token(text, text.len(), &[]).unwrap();
        assert_eq!(kind, CompletionKind::Arg(0));
        assert_eq!(start, 7);
        assert_eq!(&*query, "你好");
        assert_eq!(command, "model");

        // 未闭合的引号路径：路径里可以有空格，起点是 `@`
        let text = "@\"my notes";
        let (kind, start, query, _) = token(text, text.len(), &[]).unwrap();
        assert_eq!(kind, CompletionKind::Path);
        assert_eq!(start, 0);
        assert_eq!(&*query, "my notes");
    }

    #[test]
    fn token_reads_the_word_at_the_cursor() {
        let text = "look @src/ma and more";
        let cursor = "look @src/ma".len();
        let (kind, start, query, _) = token(text, cursor, &[]).unwrap();
        assert_eq!(kind, CompletionKind::Path);
        assert_eq!(start, 5);
        assert_eq!(&*query, "src/ma");

        // 光标在标记词中间：只看到光标前的部分
        let text = "修复 #14 已处理";
        let cursor = "修复 #14".len();
        let (kind, start, query, _) = token(text, cursor, &['#']).unwrap();
        assert_eq!(kind, CompletionKind::Marker('#'));
        assert_eq!(start, 7);
        assert_eq!(&*query, "14");
    }

    #[test]
    fn chosen_writes_paths_and_markers_as_typed() {
        assert_eq!(
            completion(CompletionKind::Path, "src/", 0, "src/")
                .chosen()
                .unwrap(),
            "@src/"
        );
        assert_eq!(
            completion(CompletionKind::Path, "src/main.rs", 0, "src/")
                .chosen()
                .unwrap(),
            "@src/main.rs "
        );
        assert_eq!(
            completion(CompletionKind::Path, "my notes.md", 0, "my")
                .chosen()
                .unwrap(),
            "@\"my notes.md\" "
        );
        assert_eq!(
            completion(CompletionKind::Path, "my dir/", 0, "my")
                .chosen()
                .unwrap(),
            "@\"my dir/"
        );
        assert_eq!(
            completion(CompletionKind::Path, "notes.md:2", 0, "notes")
                .chosen()
                .unwrap(),
            "@\"notes.md:2\" "
        );
        assert_eq!(
            completion(CompletionKind::Marker('#'), "142", 0, "14")
                .chosen()
                .unwrap(),
            "#142 "
        );
        assert_eq!(
            completion(CompletionKind::Command, "model", 0, "mo")
                .chosen()
                .unwrap(),
            "/model "
        );
        assert_eq!(
            completion(CompletionKind::Arg(0), "fast", 7, "fa")
                .chosen()
                .unwrap(),
            "fast "
        );
    }

    #[test]
    fn is_exact_tells_sending_from_completing() {
        assert!(completion(CompletionKind::Command, "model", 0, "model").is_exact());
        assert!(!completion(CompletionKind::Command, "model", 0, "mod").is_exact());
        assert!(completion(CompletionKind::Path, "src/main.rs", 0, "src/main.rs").is_exact());

        let mut open = completion(CompletionKind::Path, "src/main.rs", 0, "src/main.rs");
        open.open = true;
        assert!(!open.is_exact(), "引号没闭合时 Enter 先补完再发送");
    }

    #[test]
    fn accepting_puts_the_item_in_place_of_the_token() {
        let mut editor = Editor::default();
        editor.set("/mod");
        assert!(completion(CompletionKind::Command, "model", 0, "mod").accept(&mut editor));
        assert_eq!(editor.text(), "/model ");
        assert_eq!(editor.cursor(), 7);

        // 中文输入：`start` 与光标都是字节偏移
        let mut editor = Editor::default();
        editor.set("看 @sr");
        let mut done = completion(CompletionKind::Path, "src/", 4, "sr");
        done.items[0].text = "src/".into();
        assert!(done.accept(&mut editor));
        assert_eq!(editor.text(), "看 @src/");
        assert_eq!(editor.cursor(), 9);

        // 过期的 `start`（落在 grapheme 中间，不是字节边界）不动输入
        let mut editor = Editor::default();
        editor.set("你好");
        assert!(!completion(CompletionKind::Command, "x", 1, "").accept(&mut editor));
        assert_eq!(editor.text(), "你好");
    }

    #[test]
    fn completion_rows_mark_the_selected_item_and_show_details() {
        let theme = Theme::default();
        let list = Completion {
            kind: CompletionKind::Path,
            start: 0,
            query: "src".into(),
            items: vec![
                Candidate {
                    text: "src/".into(),
                    detail: String::new(),
                },
                Candidate {
                    text: "src/main.rs".into(),
                    detail: "入口".into(),
                },
            ],
            selected: 1,
            complete: true,
            open: false,
        };
        let rows = completion_rows(&list, 40, &theme);
        assert_eq!(rows.len(), 2, "两项都不折行");
        assert_eq!(rows[0].line.to_string(), "  @src/");
        let selected = rows[1].line.to_string();
        assert!(selected.starts_with("> @src/main.rs"), "实际 {selected:?}");
        assert!(selected.contains("入口"), "实际 {selected:?}");
        // 选中行强调色，未选中行正文色
        assert_eq!(rows[1].line.spans[0].style, theme.accent);
        assert_eq!(rows[0].line.spans[0].style, theme.fg);

        let mut empty = list.clone();
        empty.items.clear();
        assert!(completion_rows(&empty, 40, &theme).is_empty(), "没有候选就没有行");
    }

    #[test]
    fn completion_rows_wrap_at_the_width() {
        let theme = Theme::default();
        let list = completion(
            CompletionKind::Command,
            "a-very-long-command-name",
            0,
            "a-very",
        );
        let rows = completion_rows(&list, 8, &theme);
        assert!(rows.len() > 1, "8 列放不下长命令名");
        assert!(
            rows.iter().all(|row| row.line.width() <= 8),
            "每一行都不超过 8 列"
        );
    }

    #[test]
    fn walk_lists_files_and_skips_ignored_directories() {
        let root = scratch("walk");
        std::fs::write(root.join("README.md"), "").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("main.rs"), "").unwrap();
        std::fs::write(root.join("src").join("lib.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target").join("junk.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        std::fs::write(root.join("node_modules").join("pkg").join("index.js"), "").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("config"), "").unwrap();
        std::fs::write(root.join(".env"), "").unwrap();
        // 空目录不列（目录来自文件的祖先）
        std::fs::create_dir_all(root.join("empty")).unwrap();

        let paths = walk(&root).unwrap();
        assert_eq!(
            paths,
            ["README.md", "src/", "src/lib.rs", "src/main.rs"],
            "忽略隐藏名、target、node_modules 与空目录"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn walking_a_missing_root_fails() {
        let missing = std::env::temp_dir().join("rig_niri-complete-no-such-dir");
        assert!(walk(&missing).is_err());
    }

    #[test]
    fn project_root_prefers_the_shown_agents_work_dir() {
        // 没有 agent 时用 ProjectRoot；都没有就用当前目录
        let mut world = World::new();
        let mut dirs = world.query::<&WorkDir>();
        let dirs = dirs.query(&world);
        assert_eq!(project_root(None, &dirs, None), PathBuf::from("."));
        assert_eq!(
            project_root(None, &dirs, Some(&ProjectRoot(PathBuf::from("/root")))),
            PathBuf::from("/root"),
        );
    }

    #[test]
    fn the_plugin_opens_the_list_for_slash_and_marker_tokens() {
        use rig_harness::commands::Args;

        let mut app = App::new();
        app.add_plugins(CompletionPlugin)
            .init_resource::<Editor>();
        app.world_mut().spawn((
            Name::new("/model"),
            SlashCommand {
                help: "选择模型".into(),
                args: Args::None,
            },
        ));
        app.world_mut()
            .spawn(CompletionSource::marker('#').candidate("142", "崩溃"));

        // `/mod` 打开命令列表（`follow_input` 在 Layout 里跑）
        app.world_mut().resource_mut::<Editor>().set("/mod");
        app.update();
        {
            let state = app.world().resource::<CompletionState>();
            let open = state.completion().expect("/mod 应打开命令列表");
            assert_eq!(open.kind, CompletionKind::Command);
            assert_eq!(open.items[0].text, "model");
            assert_eq!(open.items[0].detail, "选择模型");
        }

        // 标记词打开插件给的候选
        app.world_mut().resource_mut::<Editor>().set("修复 #14");
        app.update();
        let state = app.world().resource::<CompletionState>();
        let open = state.completion().expect("#14 应打开标记列表");
        assert_eq!(open.kind, CompletionKind::Marker('#'));
        assert_eq!(open.items[0].text, "142");
    }

    #[test]
    fn the_completion_hint_is_the_official_line() {
        assert_eq!(COMPLETION_HINT, "Tab or Enter completes · Esc closes");
        let theme = Theme::default();
        let rows = completion_hint_rows(80, &theme);
        assert_eq!(rows.len(), 1, "80 列里提示行不折");
        assert_eq!(row_text(&rows[0]), COMPLETION_HINT);
        // 窄面板会折行，但整句仍然都在（“closes” 是最后一行的一部分）。
        let narrow = completion_hint_rows(20, &theme);
        assert!(narrow.len() > 1);
        assert!(narrow.iter().any(|row| row_text(row).contains("closes")));
    }

    /// 补全关/开：tile 高度 0 -> 6 行 -> 0。
    #[test]
    fn the_completion_tile_opens_to_six_rows_and_closes_to_zero() {
        let mut app = App::new();
        app.init_resource::<CompletionState>()
            .init_resource::<FrameTree>();
        app.world_mut()
            .run_system_once(sync_completion_tile)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<FrameTree>()
                .0
                .leaf_size(PanelId::Complete),
            Some(Size::Fixed(0.0)),
            "关着时 0 高度"
        );

        app.world_mut()
            .resource_mut::<CompletionState>()
            .show(completion(CompletionKind::Command, "help", 0, "he"));
        app.world_mut()
            .run_system_once(sync_completion_tile)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<FrameTree>()
                .0
                .leaf_size(PanelId::Complete),
            Some(Size::Fixed(completion_tile_height(true)))
        );
        assert_eq!(
            completion_tile_height(true),
            6.0 * crate::transcript::ROW_HEIGHT,
            "6 行 = 6 * 18px"
        );

        app.world_mut().resource_mut::<CompletionState>().close();
        app.world_mut()
            .run_system_once(sync_completion_tile)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<FrameTree>()
                .0
                .leaf_size(PanelId::Complete),
            Some(Size::Fixed(0.0)),
            "关回去又是 0"
        );
    }

    /// tile 打开 6 行几何：Input 上边下移 6 行、右列总高不变、叶子仍铺满。
    #[test]
    fn opening_the_tile_moves_the_input_down_without_changing_the_height() {
        use bevy::prelude::Vec2;

        let area = TileRect {
            center: Vec2::new(0.7, -0.3),
            size: Vec2::new(1.2, 0.8),
        };
        let mut tree = crate::layout::default_tree();
        let closed = tree.rects(area);
        let input_closed = closed.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;

        assert!(tree.set_leaf_size(
            PanelId::Complete,
            Size::Fixed(completion_tile_height(true))
        ));
        let open = tree.rects(area);
        let complete = open.iter().find(|(id, _)| *id == PanelId::Complete).unwrap().1;
        let input = open.iter().find(|(id, _)| *id == PanelId::Input).unwrap().1;
        let status = open.iter().find(|(id, _)| *id == PanelId::Status).unwrap().1;
        let six_rows = completion_tile_height(true) * crate::layout::PX_TO_METERS;

        assert!((complete.size.y - six_rows).abs() < 1e-6);
        // 输入/状态固定高度且贴底：打开补全时它们**不动**，让位的是顶部留白。
        assert!((input.size.y - input_closed.size.y).abs() < 1e-5, "输入高度不变");
        assert!((input.center.y - input_closed.center.y).abs() < 1e-5, "输入位置不变");
        // 输入下面依次是提示行、状态行（都是 tile）；这里只断言输入在它们上方。
        assert!(input.center.y > status.center.y, "输入在状态行上方");
        assert!(
            (complete.center.y - complete.size.y * 0.5
                - (input.center.y + input.size.y * 0.5))
                .abs()
                < 1e-5,
            "补全紧贴输入上方"
        );
        let leaves: f32 = open.iter().map(|(_, rect)| rect.size.x * rect.size.y).sum();
        assert!((leaves - area.size.x * area.size.y).abs() < 1e-5, "叶子仍铺满");
    }

    /// 补全 tile 的行槽：候选 + 官方提示行写进 `PanelRows[Complete]`。
    #[test]
    fn completion_rows_are_written_into_the_complete_panel() {
        let mut app = tile_app();
        let mut open = completion(CompletionKind::Command, "help", 0, "he");
        open.items.push(Candidate {
            text: "hello".into(),
            detail: "打招呼".into(),
        });
        open.selected = 1;
        app.world_mut()
            .resource_mut::<CompletionState>()
            .show(open);
        app.world_mut().run_system_once(draw_completion).unwrap();

        let rows = app.world().resource::<PanelRows>().get(PanelId::Complete);
        assert_eq!(rows.len(), 3, "两个候选 + 一行提示");
        let texts: Vec<String> = rows.iter().map(|row| row_text(&row.row)).collect();
        assert!(texts[0].contains("help"));
        assert!(texts[1].starts_with("> /hello"), "选中项带 > 前缀：{}", texts[1]);
        assert_eq!(texts.last().unwrap(), COMPLETION_HINT, "提示行在最后、逐字");
        assert_eq!(rows[0].key, 1, "行 key 从 1 开始，增量 diff 用");
    }

    /// 行数不超过 tile 的可见行数，提示行永远留着（候选多时截断候选）。
    #[test]
    fn the_tile_keeps_the_hint_within_its_visible_rows() {
        let theme = Theme::default();
        let mut open = completion(CompletionKind::Command, "cmd0", 0, "c");
        open.items = (0..10)
            .map(|index| Candidate {
                text: format!("cmd{index}"),
                detail: String::new(),
            })
            .collect();
        let rows = completion_tile_rows(Some(&open), 20, 4, &theme);
        assert_eq!(rows.len(), 4, "最多 4 行");
        assert!(
            row_text(rows.last().unwrap()).contains("closes"),
            "最后一行是官方的 closes"
        );
        assert!(completion_tile_rows(Some(&open), 20, 0, &theme).is_empty());
    }

    /// 关掉补全：`PanelRows[Complete]` 清空（tile 高度由另一个系统归零）。
    #[test]
    fn closing_the_completion_clears_the_tile_rows() {
        let mut app = tile_app();
        app.world_mut()
            .resource_mut::<CompletionState>()
            .show(completion(CompletionKind::Command, "help", 0, "he"));
        app.world_mut().run_system_once(draw_completion).unwrap();
        assert!(!app.world().resource::<PanelRows>().get(PanelId::Complete).is_empty());

        app.world_mut().resource_mut::<CompletionState>().close();
        app.world_mut().run_system_once(draw_completion).unwrap();
        assert!(app
            .world()
            .resource::<PanelRows>()
            .get(PanelId::Complete)
            .is_empty());
    }

    /// 补全面板的最小 App：有补全状态、行/根/区域三件套，`Complete` 面板
    /// 给 40 列 × 6 行（96×28 默认帧的右列近似）。
    fn tile_app() -> App {
        let mut app = App::new();
        app.init_resource::<PanelRows>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelAreas>()
            .init_resource::<CompletionState>();
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<PanelRoots>()
            .0
            .insert(PanelId::Complete, root);
        app.world_mut().resource_mut::<PanelAreas>().0.insert(
            root,
            PanelArea {
                cols: 40,
                rows: 6,
                ..Default::default()
            },
        );
        app
    }

    #[test]
    fn select_stops_at_the_ends() {
        let mut list = Completion {
            kind: CompletionKind::Command,
            start: 0,
            query: String::new(),
            items: vec![
                Candidate::default(),
                Candidate::default(),
                Candidate::default(),
            ],
            selected: 0,
            complete: true,
            open: false,
        };
        list.select(-1);
        assert_eq!(list.selected, 0);
        list.select(1);
        assert_eq!(list.selected, 1);
        list.select(5);
        assert_eq!(list.selected, 2);
        list.select(-1);
        assert_eq!(list.selected, 1);
    }
}
