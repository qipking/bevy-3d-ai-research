//! 提示词历史：跨会话保存，对应上游 rig-tui 0.47 的 `history.rs` 与 `jsonl.rs`。
//!
//! 上游把历史放进项目级 `PluginData` 的 `tui/history.jsonl`，用 `Files` 抽象
//! 读写。rig_niri 是纯前端，这里换成普通文件路径：[`PromptHistory::file`]
//! 是 `Option<PathBuf>`，由装配层给出（测试里是临时目录），`None` 表示只在内存
//! 里、永不落盘。文件格式仍是"一行一个 JSON 字符串"，坏行跳过并 `warn!`，不
//! panic。
//!
//! 与 [`Editor`] 的联动：上游的 `Editor` 直接拿 `PromptHistory::entries` 浏览，
//! 我们的 `Editor.history` 是普通字段（不能改 `editor.rs`），所以两边镜像：
//!
//! - 启动时（`PreStartup`）把读到的条目灌进 `Editor.history`；
//! - [`HistorySystems::Record`] 把 `Editor.history` 这一帧新增的条目（编辑器
//!   发送、清空、换模板时自己 keep 的那份）与 `PromptSubmitted`/`Sent` 里不是
//!   编辑器写的那部分按发生顺序收进历史，然后把 `Editor.history` 重建成
//!   `PromptHistory::entries` 的镜像；
//! - [`HistorySystems::Seed`] 换 agent 时把该 agent 对话里的 typed prompts 放到
//!   最新，同样镜像。
//!
//! 镜像保证 ↑ 翻到的顺序、去重与上游一致；`Editor.history_pos` 指向的下标在
//! 历史变短时由 `editor.rs` 现有的 `place` 按文本找回。

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use bevy::log::warn;
use bevy::prelude::*;
use rig_ecs::agent::{Agent, Conversation};
use rig_harness::prelude::ShownAgent;
use rig_harness::rig_tools::attach::typed_prompts;

use crate::editor::{Editor, PromptSubmitted};
use crate::FeathersSystems;

/// 历史在插件数据里的文件名：`tui/history.jsonl`（与上游一致）。
///
/// 本 crate 不读 `PluginData`（`Files` 抽象没有路径），落盘位置由装配层用
/// 这个常量拼出来，交给 [`HistoryPlugin::at`]。
pub const HISTORY_FILE: &str = "tui/history.jsonl";

/// 提示词历史。
///
/// `entries` 最旧的在前，每条只出现一次（再打一次就移到最后）。`file` 是追加
/// 保存的 JSONL 文件，`None` 表示不存盘。`unsaved` 是自上次
/// [`HistorySystems::Save`] 以来新收的条目；插件可以在 `Record` 与 `Save` 之间
/// 过滤或 [`rewrite`](Self::rewrite) 它们（比如把密钥抹掉）。
#[derive(Resource, Default)]
pub struct PromptHistory {
    /// 提示词，最旧的在前，每条一次。
    pub entries: Vec<String>,
    /// 追加保存的 JSONL 文件；`None` 只在内存里。
    pub file: Option<PathBuf>,
    /// 自上次保存以来新增的条目。
    unsaved: Vec<String>,
}

impl PromptHistory {
    /// 从 `file` 读历史；文件不存在就是空历史，不算错误。
    ///
    /// 一行一个 JSON 字符串，读不懂的行跳过并 `warn!`。重复的提示词只留最后
    /// 一次出现的位置（保持先后顺序）。读到的行数超过留存条目的两倍时把文件
    /// 重写成去重后的内容（上游 `load` 的行为），写失败只 `warn!`。
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut saved: Vec<String> = Vec::new();
        match fs::read(&path) {
            Ok(bytes) => {
                for (at, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<String>(line) {
                        Ok(prompt) => saved.push(prompt),
                        Err(failure) => {
                            warn!(
                                "跳过提示词历史 {} 第 {} 行（读不懂）：{failure}",
                                path.display(),
                                at + 1
                            );
                        }
                    }
                }
            }
            Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
            Err(failure) => {
                warn!("读不出提示词历史 {}：{failure}", path.display());
            }
        }
        // 倒着去重再翻回来：每条留最后一次出现的位置（上游 `load`）。
        let mut seen = HashSet::new();
        let mut entries: Vec<String> = (saved.iter().rev())
            .filter(|prompt| seen.insert((*prompt).clone()))
            .cloned()
            .collect();
        entries.reverse();
        // 只有重复（或坏行）让文件变大；此时顺便重写，坏行也就没了。
        if saved.len() > 2 * entries.len()
            && let Err(failure) = write(&path, &entries)
        {
            warn!("重写提示词历史 {} 失败：{failure}", path.display());
        }
        Self {
            entries,
            file: Some(path),
            unsaved: Vec::new(),
        }
    }

    /// 把 `prompt` 作为最新一条收进来（已有就移到最后），并记进待保存。
    ///
    /// 空白不算一条（上游 `record` 也 `trim` 后丢掉空白）。
    pub fn record(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return;
        }
        self.newest(prompt.to_owned());
        self.unsaved.push(prompt.to_owned());
    }

    /// 把 `prompts` 按顺序放到最新（已有就移动），不记待保存：它们是文件里
    /// 已有的对话提示词，播种不该让 [`HistorySystems::Save`] 再写一遍。
    pub fn seed(&mut self, prompts: impl IntoIterator<Item = String>) {
        for prompt in prompts {
            self.newest(prompt);
        }
    }

    /// 改写每条提示词（含还没保存的）：返回 `Some` 就替换，`None` 保持不变。
    ///
    /// 在 [`HistorySystems::Record`] 与 [`HistorySystems::Save`] 之间调用，可以
    /// 把密钥等敏感内容换成占位符而不是整条丢掉。
    pub fn rewrite(&mut self, rewrite: impl Fn(&str) -> Option<String>) {
        for prompt in self.entries.iter_mut().chain(&mut self.unsaved) {
            if let Some(rewritten) = rewrite(prompt) {
                *prompt = rewritten;
            }
        }
    }

    /// 把 `prompt` 放到最新，已有的旧位置删掉。
    fn newest(&mut self, prompt: String) {
        self.entries.retain(|entry| *entry != prompt);
        self.entries.push(prompt);
    }
}

/// 把 `entries` 整个重写进 `path`，一行一个 JSON 字符串。
fn write(path: &Path, entries: &[String]) -> io::Result<()> {
    let mut text = String::new();
    for prompt in entries {
        text.push_str(&serde_json::to_string(prompt).map_err(io::Error::other)?);
        text.push('\n');
    }
    create_parent(path)?;
    fs::write(path, text)
}

/// 把 `prompts` 逐行追加到 `path`。
fn append<'a>(path: &Path, prompts: impl IntoIterator<Item = &'a String>) -> io::Result<()> {
    let mut text = String::new();
    for prompt in prompts {
        text.push_str(&serde_json::to_string(prompt).map_err(io::Error::other)?);
        text.push('\n');
    }
    if text.is_empty() {
        return Ok(());
    }
    create_parent(path)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())
}

/// 建出 `path` 的父目录（相对文件名没有父目录时什么也不做）。
fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// 历史系统集：`Seed` 在按键前，`Record` 在按键后，`Save` 在 `Record` 之后。
///
/// 插件可以插在中间做过滤或改写（上游同样的钩子）：
///
/// ```text
/// app.add_systems(Update, scrub.after(HistorySystems::Record).before(HistorySystems::Save));
/// ```
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistorySystems {
    /// 把这一帧 `Editor` 发送/丢弃的文本与 `PromptSubmitted`/`Sent` 里剩下的
    /// 提示词按发生顺序收进 [`PromptHistory`]，并把 `Editor.history` 重建成
    /// 镜像。排在 [`FeathersSystems::Prepare`] 的按键处理之后。
    Record,
    /// 把自上次保存以来新收、而且还在 `entries` 里的条目追加到文件。
    Save,
    /// 换 agent 后把它对话里的 typed prompts 放到最新。排在按键之前。
    Seed,
}

/// 一条被发送的提示词（上游 `Sent`）：输入发送时写一条；插件替用户发送时也
/// 可以写一条，[`HistorySystems::Record`] 会把它收进 [`PromptHistory`]。
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct Sent(pub String);

/// `Editor.history` 里已经收进 [`PromptHistory`] 的条数。
#[derive(Resource, Default)]
struct Recorded(usize);

/// `PreStartup`：读 [`PromptHistory::file`]，把条目灌进 [`Editor`]（↑ 能翻到）。
fn load(
    mut history: ResMut<PromptHistory>,
    editor: Option<ResMut<Editor>>,
    mut recorded: ResMut<Recorded>,
) {
    if let Some(file) = history.file.clone() {
        history.entries = PromptHistory::load(file).entries;
    }
    let Some(mut editor) = editor else {
        return;
    };
    editor.history.clone_from(&history.entries);
    recorded.0 = editor.history.len();
}

/// 把编辑器这一帧新 keep 的文本（发送的、清空丢掉的）与 `PromptSubmitted` /
/// `Sent` 里不是编辑器写的那部分按发生顺序收进历史，然后把 `Editor.history`
/// 重建成 `entries` 的镜像。
///
/// 编辑器自己发的提示词同时出现在 `Editor.history` 和 `PromptSubmitted` 里：
/// 先按文本出现次数把消息抵掉，同一条只收一次。
fn record(
    mut submitted: MessageReader<PromptSubmitted>,
    mut sent: MessageReader<Sent>,
    mut editor: Option<ResMut<Editor>>,
    mut history: ResMut<PromptHistory>,
    mut recorded: ResMut<Recorded>,
) {
    let mut kept: Vec<String> = Vec::new();
    if let Some(editor) = editor.as_mut() {
        // `Editor.history` 只会追加；真被外部换短了就从 0 再收一遍。
        if recorded.0 > editor.history.len() {
            recorded.0 = 0;
        }
        kept.extend(editor.history[recorded.0..].iter().cloned());
        recorded.0 = editor.history.len();
    }
    // 编辑器写过的那几条先抵掉；剩下的消息（别的插件替用户发的）排在后面。
    let mut unmatched = kept.clone();
    let mut others: Vec<String> = Vec::new();
    let mut messages = submitted
        .read()
        .map(|PromptSubmitted(text)| text.clone())
        .chain(sent.read().map(|Sent(text)| text.clone()));
    for text in messages.by_ref() {
        match unmatched
            .iter()
            .position(|kept| kept.trim() == text.trim())
        {
            Some(at) => {
                unmatched.swap_remove(at);
            }
            None => others.push(text),
        }
    }
    if kept.is_empty() && others.is_empty() {
        return;
    }
    for text in kept.iter().chain(&others) {
        history.record(text.clone());
    }
    if let Some(editor) = editor.as_mut() {
        if editor.history != history.entries {
            editor.history.clone_from(&history.entries);
        }
        recorded.0 = editor.history.len();
    }
}

/// 把自上次保存以来新收、而且还在 `entries` 里的条目追加到文件。
///
/// 插件在 `Record` 与 `Save` 之间删掉的条目不会写进文件。`file: None` 时待保存
/// 条目直接丢掉（上游同样不攒着），写失败只 `warn!`。
fn save(mut history: ResMut<PromptHistory>) {
    if history.unsaved.is_empty() {
        return;
    }
    let unsaved = std::mem::take(&mut history.unsaved);
    let kept: Vec<&String> = unsaved
        .iter()
        .filter(|text| history.entries.contains(*text))
        .collect();
    let Some(file) = history.file.clone() else {
        return;
    };
    if kept.is_empty() {
        return;
    }
    if let Err(failure) = append(&file, kept) {
        warn!("保存提示词历史 {} 失败：{failure}", file.display());
    }
}

/// 让 [`Editor::history`] 跟上 `entries`。
///
/// [`HistorySystems::Record`] 已经镜像过一次；这个系统排在 `Save` 里、过滤插件
/// 之后，所以插件在 `Record` 与 `Save` 之间删掉/改写的条目，↑ 下一帧（同一个
/// 帧结束前）就看不到了，与上游直接浏览 `entries` 的行为一致。
fn mirror(
    editor: Option<ResMut<Editor>>,
    history: Res<PromptHistory>,
    mut recorded: ResMut<Recorded>,
) {
    let Some(mut editor) = editor else {
        return;
    };
    if editor.history != history.entries {
        editor.history.clone_from(&history.entries);
    }
    recorded.0 = editor.history.len();
}

/// 换 agent 时把它对话里用户亲手打出的提示词（`typed_prompts`）按顺序放到
/// 最新，并镜像进 [`Editor`]；不记待保存，因为文件里已经有它们。
fn seed_shown(
    shown: Option<Res<ShownAgent>>,
    conversations: Query<&Conversation, With<Agent>>,
    mut seeded: Local<Option<Entity>>,
    mut history: ResMut<PromptHistory>,
    mut editor: Option<ResMut<Editor>>,
    mut recorded: ResMut<Recorded>,
) {
    let Some(shown) = shown else {
        return;
    };
    if *seeded == shown.0 {
        return;
    }
    let Some(agent) = shown.0 else {
        return;
    };
    let Ok(conversation) = conversations.get(agent) else {
        return;
    };
    *seeded = Some(agent);
    let prompts: Vec<String> = typed_prompts(conversation).collect();
    history.seed(prompts);
    if let Some(editor) = editor.as_mut() {
        editor.history.clone_from(&history.entries);
        recorded.0 = editor.history.len();
    }
}

/// 提示词历史插件：启动加载、每帧记录与追加保存、换 agent 时播种。
///
/// 历史文件由装配层决定：上游放在项目的 plugin data 里，这里是普通路径，
/// 可以用 [`HISTORY_FILE`] 拼出来：
///
/// ```no_run
/// use bevy::prelude::*;
/// use rig_niri::history::{HISTORY_FILE, HistoryPlugin};
///
/// let mut app = App::new();
/// app.add_plugins(HistoryPlugin::at(
///     std::path::Path::new("/somewhere/project").join(HISTORY_FILE),
/// ));
/// ```
///
/// 用 [`HistoryPlugin::new`] 就不落盘（只用内存）。插件**不要求** `PluginData`
/// 存在，也不假设 `Editor`/`ShownAgent` 已插入：缺了都是优雅降级。
pub struct HistoryPlugin {
    file: Option<PathBuf>,
}

impl HistoryPlugin {
    /// 只在内存里记历史，不落盘。
    pub fn new() -> Self {
        Self { file: None }
    }

    /// 历史追加保存到 `file`（JSONL，一行一个提示词），启动时先读它。
    pub fn at(file: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(file.into()),
        }
    }
}

impl Default for HistoryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for HistoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Sent>()
            .add_message::<PromptSubmitted>();
        if !app.world().contains_resource::<PromptHistory>() {
            let mut history = PromptHistory::default();
            history.file.clone_from(&self.file);
            app.insert_resource(history);
        }
        app.init_resource::<Recorded>();
        // Seed 在按键之前，Record 在按键之后，Save 在 Record 之后。
        app.configure_sets(
            Update,
            (
                HistorySystems::Seed.before(FeathersSystems::Prepare),
                HistorySystems::Record.after(FeathersSystems::Prepare),
                HistorySystems::Save.after(HistorySystems::Record),
            ),
        );
        app.add_systems(PreStartup, load);
        app.add_systems(
            Update,
            (
                seed_shown.in_set(HistorySystems::Seed),
                record.in_set(HistorySystems::Record),
            )
                .chain(),
        );
        app.add_systems(
            Update,
            (save, mirror).chain().in_set(HistorySystems::Save),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rig_core::completion::Message;
    use rig_core::conversation::Conversation as Talk;

    /// 独占的临时目录；`Drop` 时删掉（断言失败也清理）。
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let at = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "rig_niri-history-{name}-{}-{at}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// 这个目录里的历史文件。
        fn file(&self) -> PathBuf {
            self.0.join(HISTORY_FILE)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// 一个只装历史插件的 app（编辑器已就位，首帧的 `PreStartup` 已经跑过）。
    fn app(plugin: HistoryPlugin) -> App {
        let mut app = App::new();
        app.add_plugins(plugin).init_resource::<Editor>();
        app.update();
        app
    }

    /// 历史文件的文本（没有文件就是空）。
    fn file_text(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    /// 往 app 里写一条发送的提示词并跑一帧。
    fn send(app: &mut App, prompt: &str) {
        app.world_mut()
            .write_message(PromptSubmitted(prompt.to_owned()));
        app.update();
    }

    #[test]
    fn a_saved_history_is_read_back_in_order_and_reaches_the_editor() {
        let scratch = Scratch::new("roundtrip");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        for prompt in ["first", "second", "third"] {
            send(&mut app, prompt);
        }
        assert_eq!(PromptHistory::load(file.clone()).entries, ["first", "second", "third"]);

        // 下一个会话：读回来，↑ 能翻到。
        let again = self::app(HistoryPlugin::at(file.clone()));
        let history = again.world().resource::<PromptHistory>();
        assert_eq!(history.entries, ["first", "second", "third"]);
        assert_eq!(
            again.world().resource::<Editor>().history,
            ["first", "second", "third"],
            "加载完把条目灌进 Editor.history"
        );
    }

    #[test]
    fn loading_dedupes_each_prompt_at_its_last_place() {
        let scratch = Scratch::new("dedupe");
        let file = scratch.file();
        create_parent(&file).unwrap();
        fs::write(&file, "\"a\"\n\"b\"\n\"a\"\n\"c\"\n").unwrap();
        let history = PromptHistory::load(file.clone());
        assert_eq!(
            history.entries,
            ["b", "a", "c"],
            "重复的 a 只留最后一次出现的位置"
        );
        assert_eq!(history.file.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn loading_skips_lines_that_are_not_json_strings() {
        let scratch = Scratch::new("broken");
        let file = scratch.file();
        create_parent(&file).unwrap();
        // 第二行不是 JSON，第三行是数字不是字符串，末尾是空行。
        fs::write(&file, "\"one\"\nnot json\n42\n\"two\"\n\n").unwrap();
        let history = PromptHistory::load(file.clone());
        assert_eq!(
            history.entries,
            ["one", "two"],
            "坏行跳过，不 panic，好行顺序不变"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_history() {
        let scratch = Scratch::new("missing");
        let file = scratch.file();
        let history = PromptHistory::load(file.clone());
        assert!(history.entries.is_empty(), "文件不存在不算错误");
        assert_eq!(history.file.as_deref(), Some(file.as_path()));
        assert_eq!(
            fs::read_dir(&scratch.0).unwrap().count(),
            0,
            "读一个不存在的文件不该建出目录或文件"
        );
    }

    #[test]
    fn record_keeps_unsent_text_and_sent_prompts_in_order() {
        let mut app = self::app(HistoryPlugin::new());
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.set("被清掉的草稿");
        editor.clear();
        drop(editor);
        app.world_mut()
            .write_message(PromptSubmitted("发送的提示词".to_owned()));
        app.update();
        let history = app.world().resource::<PromptHistory>();
        assert_eq!(history.entries, ["被清掉的草稿", "发送的提示词"]);
        assert_eq!(
            app.world().resource::<Editor>().history,
            history.entries,
            "编辑器镜像同序"
        );
    }

    #[test]
    fn a_prompt_the_editor_sent_is_recorded_once() {
        let mut app = self::app(HistoryPlugin::new());
        // 模拟 `act(Send)`：先 `sent()` 记进编辑器历史，再写事件。
        app.world_mut()
            .resource_mut::<Editor>()
            .sent("只收一次".to_owned());
        app.world_mut()
            .write_message(PromptSubmitted("只收一次".to_owned()));
        app.update();
        assert_eq!(
            app.world().resource::<PromptHistory>().entries,
            ["只收一次"],
            "编辑器的历史与 PromptSubmitted 是同一条，不重复收"
        );
    }

    #[test]
    fn save_appends_only_what_is_new() {
        let scratch = Scratch::new("append");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        send(&mut app, "one");
        assert_eq!(file_text(&file), "\"one\"\n");
        app.update();
        assert_eq!(file_text(&file), "\"one\"\n", "没有新条目就不再写");
        send(&mut app, "two");
        assert_eq!(file_text(&file), "\"one\"\n\"two\"\n", "只追加新的");
        assert_eq!(PromptHistory::load(file).entries, ["one", "two"]);
    }

    #[test]
    fn a_repeated_prompt_moves_to_the_newest_and_load_dedupes_the_file() {
        let scratch = Scratch::new("repeat");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        for prompt in ["fix it", "test it", "fix it"] {
            send(&mut app, prompt);
        }
        assert_eq!(
            app.world().resource::<PromptHistory>().entries,
            ["test it", "fix it"],
            "再打一次就移到最新，条目里只有一条"
        );
        assert_eq!(
            file_text(&file).lines().count(),
            3,
            "保存只追加：文件里留了重复行"
        );
        assert_eq!(
            PromptHistory::load(file).entries,
            ["test it", "fix it"],
            "下次加载时去重"
        );
    }

    #[test]
    fn seeding_puts_another_agents_typed_prompts_newest_and_does_not_save_them() {
        let scratch = Scratch::new("seed");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        app.world_mut().resource_mut::<PromptHistory>().entries = vec!["更早的".to_owned()];
        let agent = {
            let world = app.world_mut();
            let mut talk = Talk::new();
            talk.push(Message::user("第一句"));
            talk.push(Message::assistant("好"));
            talk.push(Message::user("第二句"));
            rig_ecs::spawn::spawn_root(world, Conversation(talk)).id()
        };
        app.insert_resource(ShownAgent(Some(agent)));
        app.update();
        let history = app.world().resource::<PromptHistory>();
        assert_eq!(history.entries, ["更早的", "第一句", "第二句"]);
        assert!(history.unsaved.is_empty(), "播种的条目不算新条目");
        assert_eq!(
            app.world().resource::<Editor>().history,
            history.entries,
            "播种后 ↑ 立刻能翻到"
        );
        assert_eq!(file_text(&file), "", "播种不重复落盘");

        // 同一个 agent 不再播种。
        app.update();
        assert_eq!(
            app.world().resource::<PromptHistory>().entries,
            ["更早的", "第一句", "第二句"]
        );
    }

    #[test]
    fn without_a_file_nothing_is_written() {
        let scratch = Scratch::new("memory");
        let mut app = self::app(HistoryPlugin::new());
        assert!(app.world().resource::<PromptHistory>().file.is_none());
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.set("只在内存里");
        editor.clear();
        drop(editor);
        app.update();
        let history = app.world().resource::<PromptHistory>();
        assert_eq!(history.entries, ["只在内存里"]);
        assert!(history.unsaved.is_empty(), "没有文件就丢掉待保存条目");
        assert_eq!(
            fs::read_dir(&scratch.0).unwrap().count(),
            0,
            "file: None 不落盘"
        );
    }

    #[test]
    fn a_filter_between_record_and_save_keeps_a_secret_out_of_the_file() {
        /// 看起来像密钥的提示词整条丢掉（上游同样的过滤钩子）。
        fn scrub(mut history: ResMut<PromptHistory>) {
            history.entries.retain(|prompt| !prompt.contains("sk-"));
        }

        let scratch = Scratch::new("filter");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        app.add_systems(
            Update,
            scrub
                .after(HistorySystems::Record)
                .before(HistorySystems::Save),
        );
        for prompt in ["hello", "my key is sk-1", "bye"] {
            send(&mut app, prompt);
        }
        assert_eq!(
            app.world().resource::<PromptHistory>().entries,
            ["hello", "bye"]
        );
        assert_eq!(file_text(&file), "\"hello\"\n\"bye\"\n");
        assert_eq!(
            app.world().resource::<Editor>().history,
            ["hello", "bye"],
            "过滤插件删掉的条目，↑ 也翻不到"
        );
    }

    #[test]
    fn rewriting_before_the_save_keeps_the_prompt_redacted() {
        fn redact(mut history: ResMut<PromptHistory>) {
            history.rewrite(|prompt| Some(prompt.replace("sk-1", "‹SECRET›")));
        }

        let scratch = Scratch::new("rewrite");
        let file = scratch.file();
        let mut app = self::app(HistoryPlugin::at(file.clone()));
        app.add_systems(
            Update,
            redact
                .after(HistorySystems::Record)
                .before(HistorySystems::Save),
        );
        send(&mut app, "my key is sk-1");
        let kept = ["my key is ‹SECRET›"];
        assert_eq!(app.world().resource::<PromptHistory>().entries, kept);
        assert_eq!(PromptHistory::load(file).entries, kept);
    }
}
