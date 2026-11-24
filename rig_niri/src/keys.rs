//! 键位表：移植上游 rig-tui 0.47 的 `src/keys.rs`。
//!
//! 上游的键位 IR 是 `crossterm::event::{KeyCode, KeyModifiers}`；这里不依赖
//! crossterm，改用 bevy 的 [`Key`]（逻辑键，字符键带字符）加本文件的
//! [`Modifiers`]。bevy 的 `KeyCode` 没有 `Char` 变体，所以 [`KeyPress`] 以
//! [`Key`] 为准；[`press_of`]/[`KeyPress::of`] 做 `KeyboardInput`（同时带
//! `key_code` 与 `logical_key`）到 [`KeyPress`] 的转换，`Key` 无法识别时回退到
//! `KeyCode` 的映射。
//!
//! 与上游一致的部分：
//! - [`built_in`] 的 43 条内置绑定（上游 `keys.rs:570`–`:650`；PARITY 写的
//!   “45 条”对 0.47.0 的源码不准确，见测试）；
//! - [`InputAction`] 的 32 个动作（从 `Clear` 到 `DismissInline`，含
//!   `AnswerInline(u8)`；PARITY 写的“19 个”是旧版本数字）；
//! - [`KeyBinding`] / [`KeyScope`] / [`KeyFocus`] / [`KeyPressed`] /
//!   [`Keys`] / [`find`] / 多键前缀匹配 / 作用域遮蔽；
//! - [`add_built_in`] 额外生成的两类绑定：转写区 Esc（`Deselect`）与
//!   Alt+1..9、Alt+0 的内联回答键（先挂 `Disabled`，本 crate 还没有内联问题
//!   的显示，所以它们暂时不会启用）。
//!
//! 未移植：`run_command`（要 rig-harness 的命令总线）、`Reflect` 与
//! `tell_refused` 的 Notice 展示；拒绝绑定时只记 [`RefusedKeys`] 并 `warn!`。

use std::fmt;
use std::str::FromStr;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::system::IntoObserverSystem;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonInput;
use bevy::prelude::*;

/// 修饰键：上游 `crossterm::KeyModifiers` 的等价物。
///
/// 字符键的 Shift 由字符本身携带（`A` 就是 Shift+a），与上游
/// [`KeyPress::new`] 的归一化一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    /// 没有修饰键。
    pub const NONE: Self = Self(0);
    /// Shift。
    pub const SHIFT: Self = Self(1);
    /// Ctrl。
    pub const CONTROL: Self = Self(2);
    /// Alt（macOS 的 Option）。
    pub const ALT: Self = Self(4);
    /// Super（Windows 键 / Command）。
    pub const SUPER: Self = Self(8);

    /// 是否包含 `other` 的每一位。
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// 是否同时包含 `other` 的任意一位。
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// 是否没有任何修饰键。
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// 去掉 `other` 里的位。
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// 加上 `other` 的位。
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// 去掉 `other` 的位。
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// 从 bevy 的按键状态读出当前按住的修饰键。
    ///
    /// 窗口侧一次 [`KeyboardInput`] 不带修饰键字段，所以用
    /// [`ButtonInput<KeyCode>`] 的当前状态补齐；同一帧里先按修饰键再按普通键
    /// 的情况由 [`crate::editor::keys`] 逐事件跟踪。
    pub fn from_held(held: &ButtonInput<KeyCode>) -> Self {
        let mut modifiers = Self::NONE;
        if held.pressed(KeyCode::ShiftLeft) || held.pressed(KeyCode::ShiftRight) {
            modifiers.insert(Self::SHIFT);
        }
        if held.pressed(KeyCode::ControlLeft) || held.pressed(KeyCode::ControlRight) {
            modifiers.insert(Self::CONTROL);
        }
        if held.pressed(KeyCode::AltLeft) || held.pressed(KeyCode::AltRight) {
            modifiers.insert(Self::ALT);
        }
        if held.pressed(KeyCode::SuperLeft) || held.pressed(KeyCode::SuperRight) {
            modifiers.insert(Self::SUPER);
        }
        modifiers
    }

    /// `code` 是不是修饰键本身（按下/松开它只更新 [`Modifiers`]）。
    pub fn is_modifier(code: KeyCode) -> bool {
        matches!(
            code,
            KeyCode::ShiftLeft
                | KeyCode::ShiftRight
                | KeyCode::ControlLeft
                | KeyCode::ControlRight
                | KeyCode::AltLeft
                | KeyCode::AltRight
                | KeyCode::SuperLeft
                | KeyCode::SuperRight
        )
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// 一个键加修饰键，形状对齐上游 `KeyPress`。
///
/// 字符键的 Shift 在构造时去掉（`A` 就是 Shift+a）；Shift+Tab 保留 Shift，
/// 因为 bevy 的 `Key` 没有单独的 BackTab 变体。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyPress {
    /// 逻辑键（bevy `Key`；字符键是 `Character`）。
    pub key: Key,
    /// Ctrl / Alt / Shift / Super。
    pub modifiers: Modifiers,
}

impl KeyPress {
    /// `key` 加 `modifiers`，按终端上报的形式归一化。
    pub fn new(key: Key, modifiers: Modifiers) -> Self {
        let modifiers = match key {
            Key::Character(_) => modifiers.difference(Modifiers::SHIFT),
            _ => modifiers,
        };
        Self { key, modifiers }
    }

    /// 单独的 `key`。
    pub fn key(key: Key) -> Self {
        Self::new(key, Modifiers::NONE)
    }

    /// 从 bevy 物理键码构造，键码没有对应的逻辑键时返回 `None`。
    ///
    /// 这是给从上游 `KeyPress::key(KeyCode::…)` 迁移过来的插件用的桥；
    /// 字符键请用 [`Self::ctrl`] / [`Self::alt`]。
    pub fn from_code(code: KeyCode, modifiers: Modifiers) -> Option<Self> {
        key_of_code(code).map(|key| Self::new(key, modifiers))
    }

    /// Ctrl+`character`。
    pub fn ctrl(character: char) -> Self {
        Self::new(character_key(character), Modifiers::CONTROL)
    }

    /// Alt+`character`。
    pub fn alt(character: char) -> Self {
        Self::new(character_key(character), Modifiers::ALT)
    }

    /// Shift+`key`。
    pub fn shift(key: Key) -> Self {
        Self::new(key, Modifiers::SHIFT)
    }

    /// 窗口上报的事件 `event` 加上当前按住的修饰键。
    pub fn of(event: &KeyboardInput, held: &ButtonInput<KeyCode>) -> Self {
        Self::new(logical_key(event), Modifiers::from_held(held))
    }

    /// 它是否会往输入里打字符：既非 Ctrl 也非 Alt 的字符键。
    pub fn typed(&self) -> Option<char> {
        match &self.key {
            Key::Character(text)
                if !self
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER) =>
            {
                text.chars().next()
            }
            Key::Space
                if !self
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER) =>
            {
                Some(' ')
            }
            _ => None,
        }
    }

    /// 同一个非字符键去掉修饰键的形式：Ctrl+Home 在 Home 没被绑定时算 Home。
    fn bare(&self) -> Option<Self> {
        let character = matches!(self.key, Key::Character(_) | Key::Space);
        (!character && !self.modifiers.is_empty()).then(|| Self::key(self.key.clone()))
    }
}

impl fmt::Display for KeyPress {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (modifier, name) in [
            (Modifiers::CONTROL, "ctrl+"),
            (Modifiers::ALT, "alt+"),
            (Modifiers::SUPER, "super+"),
            (Modifiers::SHIFT, "shift+"),
        ] {
            if self.modifiers.contains(modifier) {
                out.write_str(name)?;
            }
        }
        out.write_str(&key_name(&self.key))
    }
}

impl FromStr for KeyPress {
    type Err = String;

    /// 读一个键：`ctrl+e`、`alt+shift+left`、`f5`、`space`、`?`。修饰键
    /// （`ctrl`/`control`、`alt`/`meta`/`option`、`shift`、`super`/`cmd`）在前，
    /// 用 `+` 连接；单独一个字符区分大小写。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::NONE;
        let mut rest = text.trim();
        while let Some((modifier, after)) =
            rest.split_once('+').filter(|(_, after)| !after.is_empty())
        {
            modifiers |= match modifier.to_lowercase().as_str() {
                "ctrl" | "control" => Modifiers::CONTROL,
                "alt" | "meta" | "option" => Modifiers::ALT,
                "shift" => Modifiers::SHIFT,
                "super" | "cmd" => Modifiers::SUPER,
                _ => return Err(format!("`{modifier}` in `{text}` is not a modifier")),
            };
            rest = after;
        }
        let mut characters = rest.chars();
        let key = match (characters.next(), characters.next()) {
            (Some(character), None) if modifiers.contains(Modifiers::SHIFT) => {
                character_key(character.to_ascii_uppercase())
            }
            (Some(character), None) => character_key(character),
            _ => key_from_name(&rest.to_lowercase())
                .ok_or_else(|| format!("`{rest}` in `{text}` is not a key"))?,
        };
        let mut modifiers = modifiers;
        if rest.eq_ignore_ascii_case("backtab") {
            modifiers.insert(Modifiers::SHIFT);
        }
        Ok(Self::new(key, modifiers))
    }
}

/// 键或依次按下的多键序列，如 `ctrl+x ctrl+e`。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Keys(Vec<KeyPress>);

impl Keys {
    /// 按顺序排列的按键。
    pub fn presses(&self) -> &[KeyPress] {
        &self.0
    }

    /// 这串键是否以 `presses` 开头（或正好等于它）。
    fn starts_with(&self, presses: &[KeyPress]) -> bool {
        self.0.starts_with(presses)
    }
}

impl From<KeyPress> for Keys {
    fn from(press: KeyPress) -> Self {
        Self(vec![press])
    }
}

impl<const N: usize> From<[KeyPress; N]> for Keys {
    fn from(presses: [KeyPress; N]) -> Self {
        Self(presses.to_vec())
    }
}

impl FromStr for Keys {
    type Err = String;

    /// 空格分隔的多个键，每个按 [`KeyPress`] 的规则读。
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let presses = text
            .split_whitespace()
            .map(KeyPress::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        if presses.is_empty() {
            return Err("no key given".to_owned());
        }
        Ok(Self(presses))
    }
}

impl fmt::Display for Keys {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, press) in self.0.iter().enumerate() {
            if index > 0 {
                out.write_str(" ")?;
            }
            write!(out, "{press}")?;
        }
        Ok(())
    }
}

/// [`KeyBinding`] 生效的位置，对齐上游 `KeyScope`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyScope {
    /// 哪里都生效，只要没有 picker 或问题打开。
    #[default]
    Global,
    /// 输入框里、没有面板拿 [`KeyFocus`] 时：本 crate 的内置编辑键。
    /// Global 或 Transcript 的同名绑定会遮住它。
    Input,
    /// 转写区里，选中了某条时。它的绑定排在 Global 之后、Input 之前；
    /// 本 crate 还没有读转写状态，所以暂时没有系统会切到这个作用域。
    Transcript,
    /// 拿到 [`KeyFocus`] 的这个面板里。
    Panel(Entity),
}

impl KeyScope {
    /// 两个绑定是否能在同一个键上共存：同作用域不能，Global 与面板/转写
    /// 不能；Input 与 Global/Transcript 能（后者遮住前者）。
    fn overlaps(self, other: Self) -> bool {
        match (self, other) {
            (Self::Global | Self::Transcript, Self::Input)
            | (Self::Input, Self::Global | Self::Transcript) => false,
            (Self::Global, _) | (_, Self::Global) => true,
            _ => self == other,
        }
    }

    /// 键归 `self` 作用域时，依次要查的作用域：先是 Global，再是自己；
    /// 读转写时再查 Input。
    pub(crate) fn applying(self) -> impl Iterator<Item = Self> {
        let reading = (self == Self::Transcript).then_some(Self::Input);
        [Self::Global, self].into_iter().chain(reading)
    }
}

/// 绑在一组键上的东西，挂在名为 `key:<keys>` 的实体上；加 `Disabled`
/// 就释放它的键。
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    /// 键。
    pub keys: Keys,
    /// 它做什么，几个词，给列表看。
    pub label: String,
    /// 在哪个作用域生效。
    pub scope: KeyScope,
    /// 松开键时是否也触发 [`KeyPressed`]（`released`），给按住说话之类用。
    /// 按住不放时只触发一次，不随重复事件重复触发；多键序列听不到松开。
    pub releases: bool,
}

impl KeyBinding {
    /// `keys` 绑到任何地方，`label` 说明它做什么。
    pub fn new(keys: impl Into<Keys>, label: impl Into<String>) -> Self {
        Self {
            keys: keys.into(),
            label: label.into(),
            scope: KeyScope::Global,
            releases: false,
        }
    }

    /// 只在 `scope` 生效。
    pub fn in_scope(mut self, scope: KeyScope) -> Self {
        self.scope = scope;
        self
    }

    /// 键松开时也触发一次。
    pub fn with_releases(mut self) -> Self {
        self.releases = true;
        self
    }

    /// 两个绑定能不能同时存在：作用域重叠，且一方是另一方的键或前缀。
    pub fn conflicts(&self, other: &Self) -> bool {
        self.scope.overlaps(other.scope)
            && (self.keys.starts_with(other.keys.presses())
                || other.keys.starts_with(self.keys.presses()))
    }
}

/// 绑定的键按下（或对 [`KeyBinding::releases`] 松开）时，在绑定实体上触发。
#[derive(EntityEvent, Clone, Copy, Debug)]
pub struct KeyPressed {
    /// 绑定的实体。
    pub entity: Entity,
    /// 显示中的 agent；本 crate 还没有接 agent，先留 `None`。
    pub agent: Option<Entity>,
    /// 是松开而不是按下。
    pub released: bool,
}

/// 拿键的面板；`None` 表示输入框。本 crate 暂时只有输入框会拿键，
/// 面板要让出/收回时可以改它。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyFocus(pub Option<Entity>);

/// 在 [`App`] 上挂键位绑定。
pub trait AppKeyBindingsExt {
    /// 生成 `binding` 与 `on_press`（监听它触发的 [`KeyPressed`] 的观察者），
    /// 返回实体；与已有绑定冲突时不生成，返回 `None`，原因记进
    /// [`RefusedKeys`]。
    fn add_key_binding<M>(
        &mut self,
        binding: KeyBinding,
        on_press: impl IntoObserverSystem<KeyPressed, M>,
    ) -> Option<Entity>;
}

impl AppKeyBindingsExt for App {
    fn add_key_binding<M>(
        &mut self,
        binding: KeyBinding,
        on_press: impl IntoObserverSystem<KeyPressed, M>,
    ) -> Option<Entity> {
        let mut bound = bind(self.world_mut(), binding)?;
        bound.observe(on_press);
        Some(bound.id())
    }
}

/// 生成 `binding`，除非已有绑定与它冲突：已有的留下，新的拒绝并 `warn!`。
fn bind(world: &mut World, binding: KeyBinding) -> Option<EntityWorldMut<'_>> {
    let mut bindings = world.query::<&KeyBinding>();
    if let Some(taken) = bindings.iter(world).find(|bound| bound.conflicts(&binding)) {
        let why = format!(
            "Key `{}` ({}) is not bound: `{}` ({}) has it.",
            binding.keys, binding.label, taken.keys, taken.label
        );
        warn!("{why} Insert `Disabled` on that binding to replace it.");
        world.get_resource_or_init::<RefusedKeys>().0.push(why);
        return None;
    }
    let name = Name::new(format!("key:{}", binding.keys));
    Some(world.spawn((name, binding)))
}

/// [`bind`] 拒绝绑定时的原因，供前端展示。
#[derive(Resource, Default, Debug)]
pub struct RefusedKeys(pub Vec<String>);

/// 内置动作，对齐上游 `InputAction`（0.47.0 的 32 个变体全在）。
///
/// 挂在 [`KeyBinding`] 实体上时，按键由视图自己执行；没有这个组件时触发
/// [`KeyPressed`]。插件也可以直接 spawn 一个带 `InputAction` 的绑定来重新
/// 绑定内置动作。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAction {
    /// 清空输入。
    Clear,
    /// 输入为空时退出（本 crate 还没有退出通道，先留空实现）。
    Exit,
    /// 打一个换行。
    NewLine,
    /// 光标所在行的行首。
    LineStart,
    /// 光标所在行的行尾。
    LineEnd,
    /// 后移一个字符。
    Left,
    /// 前移一个字符。
    Right,
    /// 光标前一个词的开头。
    WordLeft,
    /// 光标后一个词的结尾。
    WordRight,
    /// 删光标前一个字符。
    Backspace,
    /// 删光标后一个字符。
    Delete,
    /// 删光标前一个词。
    DeleteWordBefore,
    /// 删光标后一个词。
    DeleteWordAfter,
    /// 删到行首；已经在行首时删掉前面的换行。
    DeleteToLineStart,
    /// 删到行尾；已经在行尾时删掉后面的换行。
    DeleteToLineEnd,
    /// 上一行，否则历史里更早的一条（Ctrl+P）。
    LineUp,
    /// 下一行，否则历史里更晚的一条（Ctrl+N）。
    LineDown,
    /// 补全的上一项，否则上一行，否则第一行行首，否则更早的历史。
    Up,
    /// 补全的下一项，否则更晚的历史，否则下一行，否则最后一行行尾。
    Down,
    /// 转写区上滚一行。
    ScrollUp,
    /// 转写区下滚一行。
    ScrollDown,
    /// 转写区上翻一页。
    PageUp,
    /// 转写区下翻一页。
    PageDown,
    /// 补全；本 crate 还没有补全，先当作“停止浏览历史”。
    Tab,
    /// 发送（Enter）。
    Send,
    /// 关补全，否则停 agent；本 crate 先留空实现。
    Stop,
    /// 粘贴图片；本 crate 还没有接图片剪贴板，先留空实现。
    PasteImage,
    /// 选转写区上一条。
    SelectUp,
    /// 选转写区下一条。
    SelectDown,
    /// 取消选择，把键交回输入框。
    Deselect,
    /// 用第 `number` 个选项回答内联问题（从 1 数）。
    AnswerInline(u8),
    /// 拒绝并关掉内联问题。
    DismissInline,
}

/// 内置绑定表：`(键, 说明, 动作)`，逐条照抄上游 `built_in()`。
///
/// 上游 0.47.0 的源码是 43 条；PARITY.md 里的“45 条”与源码不符，测试里
/// 按源码逐条断言。除这 43 条外，[`add_built_in`] 还会生成转写区 Esc 与
/// Alt+1..9、Alt+0 共 11 条，一共 54 个内置绑定实体。
pub fn built_in() -> Vec<(Keys, &'static str, InputAction)> {
    use InputAction as Do;
    let key = |key: Key| Keys::from(KeyPress::key(key));
    let ctrl = |character: char| Keys::from(KeyPress::ctrl(character));
    let alt = |character: char| Keys::from(KeyPress::alt(character));
    let with = |modifiers: Modifiers, key: Key| Keys::from(KeyPress::new(key, modifiers));
    let (control, meta, shift) = (Modifiers::CONTROL, Modifiers::ALT, Modifiers::SHIFT);
    vec![
        (ctrl('c'), "Clear the input; Up brings it back", Do::Clear),
        (ctrl('d'), "Quit, on an empty input", Do::Exit),
        (ctrl('j'), "New line", Do::NewLine),
        (with(shift, Key::Enter), "New line", Do::NewLine),
        (with(meta, Key::Enter), "New line", Do::NewLine),
        (ctrl('a'), "Start of the line", Do::LineStart),
        (key(Key::Home), "Start of the line", Do::LineStart),
        (ctrl('e'), "End of the line", Do::LineEnd),
        (key(Key::End), "End of the line", Do::LineEnd),
        (ctrl('b'), "Back a character", Do::Left),
        (key(Key::ArrowLeft), "Back a character", Do::Left),
        (ctrl('f'), "Forward a character", Do::Right),
        (key(Key::ArrowRight), "Forward a character", Do::Right),
        (alt('b'), "Back a word", Do::WordLeft),
        (with(control, Key::ArrowLeft), "Back a word", Do::WordLeft),
        (with(meta, Key::ArrowLeft), "Back a word", Do::WordLeft),
        (alt('f'), "Forward a word", Do::WordRight),
        (with(control, Key::ArrowRight), "Forward a word", Do::WordRight),
        (with(meta, Key::ArrowRight), "Forward a word", Do::WordRight),
        (key(Key::Backspace), "Delete back", Do::Backspace),
        (key(Key::Delete), "Delete forward", Do::Delete),
        (ctrl('w'), "Delete the word back", Do::DeleteWordBefore),
        (
            with(control, Key::Backspace),
            "Delete the word back",
            Do::DeleteWordBefore,
        ),
        (
            with(meta, Key::Backspace),
            "Delete the word back",
            Do::DeleteWordBefore,
        ),
        (alt('d'), "Delete the word forward", Do::DeleteWordAfter),
        (
            with(control, Key::Delete),
            "Delete the word forward",
            Do::DeleteWordAfter,
        ),
        (
            with(meta, Key::Delete),
            "Delete the word forward",
            Do::DeleteWordAfter,
        ),
        (ctrl('u'), "Delete to the line's start", Do::DeleteToLineStart),
        (ctrl('k'), "Delete to the line's end", Do::DeleteToLineEnd),
        (ctrl('p'), "Line up, or an earlier prompt", Do::LineUp),
        (ctrl('n'), "Line down, or a later prompt", Do::LineDown),
        (key(Key::ArrowUp), "Previous completion, line or prompt", Do::Up),
        (
            key(Key::ArrowDown),
            "Next completion, line or prompt",
            Do::Down,
        ),
        (with(shift, Key::ArrowUp), "Scroll up", Do::ScrollUp),
        (with(shift, Key::ArrowDown), "Scroll down", Do::ScrollDown),
        (key(Key::PageUp), "Scroll a page up", Do::PageUp),
        (key(Key::PageDown), "Scroll a page down", Do::PageDown),
        (key(Key::Tab), "Complete, or send after this turn", Do::Tab),
        (key(Key::Enter), "Send, or steer the running turn", Do::Send),
        (key(Key::Escape), "Close the completion, or stop the agent", Do::Stop),
        (ctrl('v'), "Paste an image", Do::PasteImage),
        (with(meta, Key::ArrowUp), "Select the item above", Do::SelectUp),
        (
            with(meta, Key::ArrowDown),
            "Select the item below",
            Do::SelectDown,
        ),
    ]
}

/// 生成视图自带的内置绑定：43 条输入键、转写区 Esc，以及先挂
/// `Disabled` 的内联回答键（Alt+1..9、Alt+0）。
pub fn add_built_in(app: &mut App) {
    app.init_resource::<KeyFocus>();
    for (keys, label, action) in built_in() {
        let binding = KeyBinding::new(keys, label).in_scope(KeyScope::Input);
        if let Some(mut bound) = bind(app.world_mut(), binding) {
            bound.insert(action);
        }
    }
    // 读转写时 Esc 把键交回输入框。
    let back = KeyBinding::new(KeyPress::key(Key::Escape), "Back to the input")
        .in_scope(KeyScope::Transcript);
    if let Some(mut bound) = bind(app.world_mut(), back) {
        bound.insert(InputAction::Deselect);
    }
    // 内联问题：Alt+1..9 选第 n 个，Alt+0 拒绝。还没有内联问题显示，
    // 所以先全部 Disabled；等有显示方时按上游 `arm_inline_keys` 摘掉。
    let inline = (1..=9u8)
        .map(|number| (number, "Answer the question beside the work"))
        .chain([(0, "Dismiss the question beside the work")]);
    for (number, label) in inline {
        let digit = char::from(b'0' + number);
        let binding = KeyBinding::new(KeyPress::alt(digit), label).in_scope(KeyScope::Input);
        let action = match number {
            0 => InputAction::DismissInline,
            number => InputAction::AnswerInline(number),
        };
        if let Some(mut bound) = bind(app.world_mut(), binding) {
            bound.insert((action, Disabled));
        }
    }
}

/// 查表用的一行：绑定实体、绑定、以及它挂的动作。
#[derive(Clone, Copy, Debug)]
pub struct BindingRef<'a> {
    /// 绑定实体。
    pub entity: Entity,
    /// 绑定。
    pub binding: &'a KeyBinding,
    /// 挂着的动作；没有就只触发 [`KeyPressed`]。
    pub action: Option<InputAction>,
}

/// [`find`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// 就是这个绑定的键。
    Binding(Entity),
    /// 是某个绑定的键的开头：等下一个键。
    Partial,
    /// 没有绑定。
    Nothing,
}

/// 在 `typed` 这些已按下的键上查 `bindings`，按 `focus` 决定作用域顺序，
/// 所以 Global 的绑定遮住 Input 的；同作用域里实体最小的赢（与绑定顺序
/// 无关）。一个非字符单键带没有任何绑定要的修饰键时，按它的无修饰键形式
/// 再查一次。
pub fn find(bindings: &[BindingRef<'_>], typed: &[KeyPress], focus: KeyScope) -> Found {
    fn exact(
        bindings: &[BindingRef<'_>],
        typed: &[KeyPress],
        scope: KeyScope,
    ) -> Option<Entity> {
        bindings
            .iter()
            .filter(|row| row.binding.scope == scope && row.binding.keys.presses() == typed)
            .map(|row| row.entity)
            .min()
    }
    fn starts(bindings: &[BindingRef<'_>], typed: &[KeyPress], scope: KeyScope) -> bool {
        bindings
            .iter()
            .any(|row| row.binding.scope == scope && row.binding.keys.starts_with(typed))
    }
    for scope in focus.applying() {
        if let Some(entity) = exact(bindings, typed, scope) {
            return Found::Binding(entity);
        }
        if starts(bindings, typed, scope) {
            return Found::Partial;
        }
    }
    let bare = match typed {
        [press] => press.bare(),
        _ => None,
    };
    bare.and_then(|bare| {
        focus
            .applying()
            .find_map(|scope| exact(bindings, &[bare.clone()], scope))
    })
    .map_or(Found::Nothing, Found::Binding)
}

/// 查 `typed` 命中哪个内置动作；插件绑定与作用域请用 [`find`]。
pub fn action_for(
    bindings: &[BindingRef<'_>],
    typed: &[KeyPress],
    focus: KeyScope,
) -> Option<InputAction> {
    match find(bindings, typed, focus) {
        Found::Binding(entity) => bindings
            .iter()
            .find(|row| row.entity == entity)
            .and_then(|row| row.action),
        _ => None,
    }
}

/// 只查 43 条内置表的便捷入口：`held` 是当前按住的键，`event` 是一次窗口
/// 按键。它不管插件绑定、多键序列和作用域，供窗口贴片与测试用；真正分派走
/// [`find`]/[`action_for`]。
pub fn action_of(held: &ButtonInput<KeyCode>, event: &KeyboardInput) -> Option<InputAction> {
    let press = press_of(event, held);
    built_in()
        .into_iter()
        .find(|(keys, _, _)| keys.presses() == std::slice::from_ref(&press))
        .map(|(_, _, action)| action)
}

/// 松开 `press` 时应该收到 `released` 的绑定：要求 `releases`、单键相等，
/// 且它的作用域在 `focus` 下适用。
pub fn released(bindings: &[BindingRef<'_>], press: &KeyPress, focus: KeyScope) -> Option<Entity> {
    bindings
        .iter()
        .find(|row| {
            row.binding.releases
                && row.binding.keys.presses() == std::slice::from_ref(press)
                && focus
                    .applying()
                    .any(|applying| applying == row.binding.scope)
        })
        .map(|row| row.entity)
}

/// 已按下、还没凑成一个绑定的键序列（上游 `PendingInput` 的最小等价物）。
#[derive(Resource, Default, Debug)]
pub struct PendingInput(pub Vec<KeyPress>);

/// 把窗口事件变成 [`KeyPress`]：逻辑键优先，`Unidentified` 时回退物理键码。
pub fn press_of(event: &KeyboardInput, held: &ButtonInput<KeyCode>) -> KeyPress {
    KeyPress::new(logical_key(event), Modifiers::from_held(held))
}

/// 用调用方逐事件维护的修饰键状态构造 [`KeyPress`]。
///
/// 输入系统在同一帧里先按修饰键、再按普通键时不能用 [`ButtonInput`] 的
/// 帧尾状态，所以自己跟踪后用这个入口。
pub fn press_with(event: &KeyboardInput, modifiers: Modifiers) -> KeyPress {
    KeyPress::new(logical_key(event), modifiers)
}

/// 窗口事件的逻辑键；bevy 给不出时按物理键码映射（字符键为 US 布局的近似）。
fn logical_key(event: &KeyboardInput) -> Key {
    match &event.logical_key {
        Key::Unidentified(_) | Key::Dead(_) => {
            key_of_code(event.key_code).unwrap_or_else(|| event.logical_key.clone())
        }
        key => key.clone(),
    }
}

/// 字符 → bevy [`Key`]。空格在 bevy 里是单独的 `Key::Space`，这里归一，
/// 否则 `ctrl(' ')` 与从 `space` 文本解析出来的键对不上。
fn character_key(character: char) -> Key {
    match character {
        ' ' => Key::Space,
        character => Key::Character(character.to_string().into()),
    }
}

/// `<key>` 的显示名，与上游 `Display` 一致（`esc` 而不是 `escape`）。
fn key_name(key: &Key) -> String {
    match key {
        Key::Character(text) if text == " " => "space".to_owned(),
        Key::Character(text) => text.to_string(),
        Key::Unidentified(_) => "?".to_owned(),
        Key::Dead(_) => "?".to_owned(),
        Key::Alt => "alt".to_owned(),
        Key::AltGraph => "altgraph".to_owned(),
        Key::CapsLock => "capslock".to_owned(),
        Key::Control => "control".to_owned(),
        Key::Fn => "fn".to_owned(),
        Key::FnLock => "fnlock".to_owned(),
        Key::NumLock => "numlock".to_owned(),
        Key::ScrollLock => "scrolllock".to_owned(),
        Key::Shift => "shift".to_owned(),
        Key::Super => "super".to_owned(),
        Key::Enter => "enter".to_owned(),
        Key::Tab => "tab".to_owned(),
        Key::Space => "space".to_owned(),
        Key::ArrowDown => "down".to_owned(),
        Key::ArrowLeft => "left".to_owned(),
        Key::ArrowRight => "right".to_owned(),
        Key::ArrowUp => "up".to_owned(),
        Key::End => "end".to_owned(),
        Key::Home => "home".to_owned(),
        Key::PageDown => "pagedown".to_owned(),
        Key::PageUp => "pageup".to_owned(),
        Key::Backspace => "backspace".to_owned(),
        Key::Delete => "delete".to_owned(),
        Key::Insert => "insert".to_owned(),
        Key::Escape => "esc".to_owned(),
        key => match f_number(key) {
            Some(number) => format!("f{number}"),
            None => "?".to_owned(),
        },
    }
}

/// 名字 → 键，与上游 `NAMES` 同表（大小写不敏感）。
fn key_from_name(name: &str) -> Option<Key> {
    let key = match name {
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backtab" => Key::Tab, // bevy 没有 BackTab，Shift 由修饰键携带
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "space" => Key::Space,
        _ => {
            let number = name.strip_prefix('f')?.parse::<u8>().ok()?;
            return (1..=24).contains(&number).then(|| f_key(number)).flatten();
        }
    };
    Some(key)
}

/// `number` 号的 F 键（1..=24）。
fn f_key(number: u8) -> Option<Key> {
    Some(match number {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        13 => Key::F13,
        14 => Key::F14,
        15 => Key::F15,
        16 => Key::F16,
        17 => Key::F17,
        18 => Key::F18,
        19 => Key::F19,
        20 => Key::F20,
        21 => Key::F21,
        22 => Key::F22,
        23 => Key::F23,
        24 => Key::F24,
        _ => return None,
    })
}

/// F 键的编号。
fn f_number(key: &Key) -> Option<u8> {
    Some(match key {
        Key::F1 => 1,
        Key::F2 => 2,
        Key::F3 => 3,
        Key::F4 => 4,
        Key::F5 => 5,
        Key::F6 => 6,
        Key::F7 => 7,
        Key::F8 => 8,
        Key::F9 => 9,
        Key::F10 => 10,
        Key::F11 => 11,
        Key::F12 => 12,
        Key::F13 => 13,
        Key::F14 => 14,
        Key::F15 => 15,
        Key::F16 => 16,
        Key::F17 => 17,
        Key::F18 => 18,
        Key::F19 => 19,
        Key::F20 => 20,
        Key::F21 => 21,
        Key::F22 => 22,
        Key::F23 => 23,
        Key::F24 => 24,
        _ => return None,
    })
}

/// bevy 物理键码 → 逻辑键（字符键按 US 布局；只用于 `logical_key` 缺失的
/// 回退与 [`KeyPress::from_code`]）。
fn key_of_code(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyA => character_key('a'),
        KeyCode::KeyB => character_key('b'),
        KeyCode::KeyC => character_key('c'),
        KeyCode::KeyD => character_key('d'),
        KeyCode::KeyE => character_key('e'),
        KeyCode::KeyF => character_key('f'),
        KeyCode::KeyG => character_key('g'),
        KeyCode::KeyH => character_key('h'),
        KeyCode::KeyI => character_key('i'),
        KeyCode::KeyJ => character_key('j'),
        KeyCode::KeyK => character_key('k'),
        KeyCode::KeyL => character_key('l'),
        KeyCode::KeyM => character_key('m'),
        KeyCode::KeyN => character_key('n'),
        KeyCode::KeyO => character_key('o'),
        KeyCode::KeyP => character_key('p'),
        KeyCode::KeyQ => character_key('q'),
        KeyCode::KeyR => character_key('r'),
        KeyCode::KeyS => character_key('s'),
        KeyCode::KeyT => character_key('t'),
        KeyCode::KeyU => character_key('u'),
        KeyCode::KeyV => character_key('v'),
        KeyCode::KeyW => character_key('w'),
        KeyCode::KeyX => character_key('x'),
        KeyCode::KeyY => character_key('y'),
        KeyCode::KeyZ => character_key('z'),
        KeyCode::Digit0 | KeyCode::Numpad0 => character_key('0'),
        KeyCode::Digit1 | KeyCode::Numpad1 => character_key('1'),
        KeyCode::Digit2 | KeyCode::Numpad2 => character_key('2'),
        KeyCode::Digit3 | KeyCode::Numpad3 => character_key('3'),
        KeyCode::Digit4 | KeyCode::Numpad4 => character_key('4'),
        KeyCode::Digit5 | KeyCode::Numpad5 => character_key('5'),
        KeyCode::Digit6 | KeyCode::Numpad6 => character_key('6'),
        KeyCode::Digit7 | KeyCode::Numpad7 => character_key('7'),
        KeyCode::Digit8 | KeyCode::Numpad8 => character_key('8'),
        KeyCode::Digit9 | KeyCode::Numpad9 => character_key('9'),
        KeyCode::Backquote => character_key('`'),
        KeyCode::Backslash => character_key('\\'),
        KeyCode::BracketLeft => character_key('['),
        KeyCode::BracketRight => character_key(']'),
        KeyCode::Comma => character_key(','),
        KeyCode::Equal => character_key('='),
        KeyCode::Minus => character_key('-'),
        KeyCode::Period => character_key('.'),
        KeyCode::Quote => character_key('\''),
        KeyCode::Semicolon => character_key(';'),
        KeyCode::Slash => character_key('/'),
        KeyCode::Space => Key::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Escape => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::F13 => Key::F13,
        KeyCode::F14 => Key::F14,
        KeyCode::F15 => Key::F15,
        KeyCode::F16 => Key::F16,
        KeyCode::F17 => Key::F17,
        KeyCode::F18 => Key::F18,
        KeyCode::F19 => Key::F19,
        KeyCode::F20 => Key::F20,
        KeyCode::F21 => Key::F21,
        KeyCode::F22 => Key::F22,
        KeyCode::F23 => Key::F23,
        KeyCode::F24 => Key::F24,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::query::Allow;
    use bevy::input::ButtonState;

    /// 上游 `built_in()` 的 43 条，逐条抄成 `(键, 说明, 动作)`。
    fn expected_built_in() -> Vec<(&'static str, &'static str, InputAction)> {
        use InputAction as Do;
        vec![
            ("ctrl+c", "Clear the input; Up brings it back", Do::Clear),
            ("ctrl+d", "Quit, on an empty input", Do::Exit),
            ("ctrl+j", "New line", Do::NewLine),
            ("shift+enter", "New line", Do::NewLine),
            ("alt+enter", "New line", Do::NewLine),
            ("ctrl+a", "Start of the line", Do::LineStart),
            ("home", "Start of the line", Do::LineStart),
            ("ctrl+e", "End of the line", Do::LineEnd),
            ("end", "End of the line", Do::LineEnd),
            ("ctrl+b", "Back a character", Do::Left),
            ("left", "Back a character", Do::Left),
            ("ctrl+f", "Forward a character", Do::Right),
            ("right", "Forward a character", Do::Right),
            ("alt+b", "Back a word", Do::WordLeft),
            ("ctrl+left", "Back a word", Do::WordLeft),
            ("alt+left", "Back a word", Do::WordLeft),
            ("alt+f", "Forward a word", Do::WordRight),
            ("ctrl+right", "Forward a word", Do::WordRight),
            ("alt+right", "Forward a word", Do::WordRight),
            ("backspace", "Delete back", Do::Backspace),
            ("delete", "Delete forward", Do::Delete),
            ("ctrl+w", "Delete the word back", Do::DeleteWordBefore),
            ("ctrl+backspace", "Delete the word back", Do::DeleteWordBefore),
            ("alt+backspace", "Delete the word back", Do::DeleteWordBefore),
            ("alt+d", "Delete the word forward", Do::DeleteWordAfter),
            ("ctrl+delete", "Delete the word forward", Do::DeleteWordAfter),
            ("alt+delete", "Delete the word forward", Do::DeleteWordAfter),
            ("ctrl+u", "Delete to the line's start", Do::DeleteToLineStart),
            ("ctrl+k", "Delete to the line's end", Do::DeleteToLineEnd),
            ("ctrl+p", "Line up, or an earlier prompt", Do::LineUp),
            ("ctrl+n", "Line down, or a later prompt", Do::LineDown),
            ("up", "Previous completion, line or prompt", Do::Up),
            ("down", "Next completion, line or prompt", Do::Down),
            ("shift+up", "Scroll up", Do::ScrollUp),
            ("shift+down", "Scroll down", Do::ScrollDown),
            ("pageup", "Scroll a page up", Do::PageUp),
            ("pagedown", "Scroll a page down", Do::PageDown),
            ("tab", "Complete, or send after this turn", Do::Tab),
            ("enter", "Send, or steer the running turn", Do::Send),
            ("esc", "Close the completion, or stop the agent", Do::Stop),
            ("ctrl+v", "Paste an image", Do::PasteImage),
            ("alt+up", "Select the item above", Do::SelectUp),
            ("alt+down", "Select the item below", Do::SelectDown),
        ]
    }

    fn press_event(key: Key, code: KeyCode) -> KeyboardInput {
        KeyboardInput {
            key_code: code,
            logical_key: key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn built_in_table_is_the_upstream_43() {
        let built = built_in();
        let expected = expected_built_in();
        assert_eq!(built.len(), 43, "上游 rig-tui 0.47.0 的 built_in() 是 43 条");
        assert_eq!(expected.len(), 43);
        for (index, ((keys, label, action), (want_keys, want_label, want_action))) in
            built.iter().zip(expected).enumerate()
        {
            assert_eq!(keys.to_string(), want_keys, "第 {index} 条的键");
            assert_eq!(*label, want_label, "第 {index} 条的说明");
            assert_eq!(*action, want_action, "第 {index} 条的动作");
        }
    }

    #[test]
    fn add_built_in_spawns_43_plus_transcript_esc_plus_10_inline() {
        let mut app = App::new();
        add_built_in(&mut app);
        // 默认查询会跳过 Disabled，所以显式 Allow 它们，数全部内置实体。
        let count = app
            .world_mut()
            .query_filtered::<&KeyBinding, Allow<Disabled>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 43 + 1 + 10);
        let enabled = app
            .world_mut()
            .query::<&KeyBinding>()
            .iter(app.world())
            .count();
        assert_eq!(enabled, 43 + 1, "Alt+0..9 先 Disabled");
    }

    #[test]
    fn add_built_in_also_binds_transcript_esc_and_the_disabled_inline_keys() {
        let mut app = App::new();
        add_built_in(&mut app);
        let mut query = app.world_mut().query_filtered::<
            (&KeyBinding, Option<&InputAction>, Has<Disabled>),
            Allow<Disabled>,
        >();
        let rows: Vec<(String, Option<InputAction>, bool, KeyScope)> = query
            .iter(app.world())
            .map(|(binding, action, disabled)| {
                (
                    binding.keys.to_string(),
                    action.copied(),
                    disabled,
                    binding.scope,
                )
            })
            .collect();

        let transcript: Vec<_> = rows
            .iter()
            .filter(|row| row.3 == KeyScope::Transcript)
            .collect();
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].0, "esc");
        assert_eq!(transcript[0].1, Some(InputAction::Deselect));
        assert!(!transcript[0].2, "转写区 Esc 是启用的");

        let inline: Vec<_> = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.1,
                    Some(InputAction::AnswerInline(_) | InputAction::DismissInline)
                )
            })
            .collect();
        assert_eq!(inline.len(), 10, "Alt+1..9 与 Alt+0");
        for (index, row) in inline.iter().enumerate() {
            let digit = if index == 9 { 0 } else { index + 1 };
            assert_eq!(row.0, format!("alt+{digit}"));
            assert!(row.2, "内联键在所有问题都关着时先 Disabled");
            assert_eq!(row.3, KeyScope::Input);
            let want = match digit {
                0 => InputAction::DismissInline,
                digit => InputAction::AnswerInline(digit as u8),
            };
            assert_eq!(row.1, Some(want));
        }
    }

    #[test]
    fn a_binding_that_asks_for_releases_is_found_on_release() {
        let entity = Entity::from_raw_u32(11).unwrap();
        let talk = KeyBinding::new(KeyPress::alt(' '), "Talk")
            .in_scope(KeyScope::Input)
            .with_releases();
        let rows = [BindingRef {
            entity,
            binding: &talk,
            action: None,
        }];
        let press = KeyPress::alt(' ');
        assert_eq!(released(&rows, &press, KeyScope::Input), Some(entity));
        assert_eq!(released(&rows, &press, KeyScope::Panel(entity)), None);
        let quiet = KeyBinding::new(KeyPress::alt(' '), "Quiet").in_scope(KeyScope::Input);
        let rows = [BindingRef {
            entity,
            binding: &quiet,
            action: None,
        }];
        assert_eq!(released(&rows, &press, KeyScope::Input), None);
    }

    #[test]
    fn keys_read_as_they_are_written() {
        for written in [
            "ctrl+x ctrl+e",
            "alt+shift+left",
            "shift+tab",
            "f5",
            "ctrl+space",
            "A",
        ] {
            let keys: Result<Keys, String> = written.parse();
            assert_eq!(keys.map(|keys| keys.to_string()).as_deref(), Ok(written));
        }
        assert_eq!(
            "Ctrl+E".parse::<Keys>(),
            Ok(Keys::from(KeyPress::ctrl('E')))
        );
        assert_eq!(
            "shift+a".parse::<Keys>(),
            Ok(Keys::from(KeyPress::key(character_key('A'))))
        );
        for wrong in ["hyper+x", "ctrl+bogus", "", "f99"] {
            assert!(wrong.parse::<Keys>().is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_press_made_from_a_window_event_keeps_its_modifiers_and_character() {
        let mut held = ButtonInput::<KeyCode>::default();
        held.press(KeyCode::ControlLeft);
        let event = press_event(character_key('c'), KeyCode::KeyC);
        let press = press_of(&event, &held);
        assert_eq!(press, KeyPress::ctrl('c'));
        assert_eq!(press.to_string(), "ctrl+c");
        assert_eq!(press.typed(), None, "Ctrl+C 不打字符");

        // Shift+a 的字符自带大小写，Shift 被归一化掉。
        let mut held = ButtonInput::<KeyCode>::default();
        held.press(KeyCode::ShiftLeft);
        let event = press_event(character_key('A'), KeyCode::KeyA);
        let press = press_of(&event, &held);
        assert_eq!(press, KeyPress::key(character_key('A')));
        assert_eq!(press.typed(), Some('A'));
        assert_eq!(press.bare(), None, "字符键不走 bare");

        // 非字符键带没人绑的修饰键时回退到无修饰形式。
        let mut held = ButtonInput::<KeyCode>::default();
        held.press(KeyCode::ControlLeft);
        let event = press_event(Key::Home, KeyCode::Home);
        let press = press_of(&event, &held);
        assert_eq!(press, KeyPress::new(Key::Home, Modifiers::CONTROL));
        assert_eq!(press.bare(), Some(KeyPress::key(Key::Home)));
    }

    #[test]
    fn action_of_resolves_the_required_built_in_keys() {
        let cases = [
            (character_key('a'), KeyCode::KeyA, Modifiers::CONTROL, InputAction::LineStart),
            (character_key('e'), KeyCode::KeyE, Modifiers::CONTROL, InputAction::LineEnd),
            (character_key('w'), KeyCode::KeyW, Modifiers::CONTROL, InputAction::DeleteWordBefore),
            (character_key('u'), KeyCode::KeyU, Modifiers::CONTROL, InputAction::DeleteToLineStart),
            (character_key('k'), KeyCode::KeyK, Modifiers::CONTROL, InputAction::DeleteToLineEnd),
            (character_key('b'), KeyCode::KeyB, Modifiers::ALT, InputAction::WordLeft),
            (character_key('f'), KeyCode::KeyF, Modifiers::ALT, InputAction::WordRight),
            (character_key('d'), KeyCode::KeyD, Modifiers::ALT, InputAction::DeleteWordAfter),
            (Key::Enter, KeyCode::Enter, Modifiers::SHIFT, InputAction::NewLine),
            (Key::Escape, KeyCode::Escape, Modifiers::NONE, InputAction::Stop),
            (Key::ArrowUp, KeyCode::ArrowUp, Modifiers::NONE, InputAction::Up),
            (Key::ArrowDown, KeyCode::ArrowDown, Modifiers::NONE, InputAction::Down),
        ];
        for (key, code, modifiers, want) in cases {
            let mut held = ButtonInput::<KeyCode>::default();
            if modifiers.contains(Modifiers::CONTROL) {
                held.press(KeyCode::ControlLeft);
            }
            if modifiers.contains(Modifiers::ALT) {
                held.press(KeyCode::AltLeft);
            }
            if modifiers.contains(Modifiers::SHIFT) {
                held.press(KeyCode::ShiftLeft);
            }
            let event = press_event(key, code);
            assert_eq!(action_of(&held, &event), Some(want));
        }
    }

    #[test]
    fn a_global_binding_shadows_the_inputs_and_scopes_do_not_conflict() {
        let global = KeyBinding::new(KeyPress::ctrl('e'), "Plugin").in_scope(KeyScope::Global);
        let input = KeyBinding::new(KeyPress::ctrl('e'), "End").in_scope(KeyScope::Input);
        assert!(
            !global.conflicts(&input),
            "Global 与 Input 不冲突：前者遮住后者"
        );
        let entity = Entity::from_raw_u32(7).unwrap();
        let rows = [
            BindingRef {
                entity: Entity::from_raw_u32(1).unwrap(),
                binding: &input,
                action: Some(InputAction::LineEnd),
            },
            BindingRef {
                entity,
                binding: &global,
                action: Some(InputAction::Clear),
            },
        ];
        assert_eq!(
            find(&rows, &[KeyPress::ctrl('e')], KeyScope::Input),
            Found::Binding(entity)
        );
        assert_eq!(
            action_for(&rows, &[KeyPress::ctrl('e')], KeyScope::Input),
            Some(InputAction::Clear)
        );

        // 同作用域同名键才冲突。
        let again = KeyBinding::new(KeyPress::ctrl('e'), "Again").in_scope(KeyScope::Input);
        assert!(input.conflicts(&again));
        assert!(!again.conflicts(&global));
        // 前缀也算冲突。
        let sequence = Keys::from([KeyPress::ctrl('x'), KeyPress::ctrl('e')]);
        let multi = KeyBinding::new(sequence, "Edit").in_scope(KeyScope::Input);
        let prefix = KeyBinding::new(KeyPress::ctrl('x'), "Prefix").in_scope(KeyScope::Input);
        assert!(prefix.conflicts(&multi));
    }

    #[test]
    fn a_multi_key_sequence_waits_for_its_last_key() {
        let binding = KeyBinding::new(
            Keys::from([KeyPress::ctrl('x'), KeyPress::ctrl('e')]),
            "Edit in $EDITOR",
        )
        .in_scope(KeyScope::Input);
        let rows = [BindingRef {
            entity: Entity::from_raw_u32(3).unwrap(),
            binding: &binding,
            action: None,
        }];
        assert_eq!(
            find(&rows, &[KeyPress::ctrl('x')], KeyScope::Input),
            Found::Partial
        );
        assert_eq!(
            find(
                &rows,
                &[KeyPress::ctrl('x'), KeyPress::ctrl('e')],
                KeyScope::Input
            ),
            Found::Binding(rows[0].entity)
        );
        assert_eq!(
            find(&rows, &[KeyPress::ctrl('x'), KeyPress::ctrl('a')], KeyScope::Input),
            Found::Nothing
        );
    }

    #[test]
    fn a_panel_binding_only_applies_with_that_panels_focus() {
        let panel = Entity::from_raw_u32(9).unwrap();
        let binding = KeyBinding::new(KeyPress::key(Key::ArrowUp), "Scroll")
            .in_scope(KeyScope::Panel(panel));
        let rows = [BindingRef {
            entity: panel,
            binding: &binding,
            action: Some(InputAction::ScrollUp),
        }];
        assert_eq!(
            find(&rows, &[KeyPress::key(Key::ArrowUp)], KeyScope::Panel(panel)),
            Found::Binding(panel)
        );
        assert_eq!(
            find(&rows, &[KeyPress::key(Key::ArrowUp)], KeyScope::Input),
            Found::Nothing
        );
    }

    #[test]
    fn a_refused_binding_is_kept_out_and_reported() {
        let mut app = App::new();
        app.init_resource::<RefusedKeys>();
        let observer = |_: On<KeyPressed>| {};
        let first = app.add_key_binding(
            KeyBinding::new(KeyPress::ctrl('o'), "Open").in_scope(KeyScope::Input),
            observer,
        );
        assert!(first.is_some());
        let second = app.add_key_binding(
            KeyBinding::new(KeyPress::ctrl('o'), "Show output").in_scope(KeyScope::Input),
            |_: On<KeyPressed>| {},
        );
        assert!(second.is_none());
        let refused = app.world().resource::<RefusedKeys>();
        assert_eq!(refused.0.len(), 1);
        assert!(refused.0[0].contains("Show output"));
        assert!(refused.0[0].contains("Open"));
        // 全局绑定与内置的 Input 绑定不冲突，可以加。
        assert!(app
            .add_key_binding(
                KeyBinding::new(KeyPress::ctrl('o'), "Global"),
                |_: On<KeyPressed>| {},
            )
            .is_some());
    }
}
