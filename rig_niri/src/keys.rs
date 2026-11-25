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
//! - [`InputAction`] 的 33 个动作（上游 0.47.0 的 32 个，加本 crate 的
//!   [`InputAction::CopySelection`]；含 `AnswerInline(u8)`，PARITY 写的
//!   “19 个”是旧版本数字）；
//! - [`KeyBinding`] / [`KeyScope`] / [`KeyFocus`] / [`KeyPressed`] /
//!   [`Keys`] / [`find`] / 多键前缀匹配 / 作用域遮蔽；
//! - [`add_built_in`] 额外生成的三类绑定：F10 的 `Ctrl+Shift+C`
//!   （[`InputAction::CopySelection`]）、转写区 Esc（`Deselect`）与
//!   Alt+1..9、Alt+0 的内联回答键（带 [`InlineKey`]，先挂 `Disabled`）。
//!   确认面板里有未回答的 `Ask` 时，[`arm_inline_keys`] 摘掉 `Disabled`
//!   启用它们；问题答完/被拒后挂回去——和上游 `keys.rs::arm_inline_keys`
//!   同一条规则。
//!
//! 已移植：[`run_command`]（上游 `keys.rs:455`）把绑定到的键变成"对显示中的
//! agent 跑一条斜杠命令"，内部走 rig-harness 的 `run_chosen`：键按下的命令不
//! 是用户打的，所以被拒时只写一条错误 [`Notice`](rig_ecs::prelude::Notice)，
//! 不回填输入（上游 `commands.rs` 对 `run_chosen` 的规则）。
//!
//! 未移植：`Reflect` 与 `tell_refused` 的 Notice 展示；拒绝绑定时只记
//! [`RefusedKeys`] 并 `warn!`。

use std::fmt;
use std::str::FromStr;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::system::IntoObserverSystem;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonInput;
use bevy::prelude::*;
use rig_ecs::prelude::{ForAgent, Outcome};
use rig_harness::commands::run_chosen;
use rig_harness::main_agent::ShownAgent;
use rig_harness::prelude::Ask;

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
    /// 显示中的 agent：按下这一帧 [`ShownAgent`] 指向的实体（应用层在切
    /// 工作区时更新它），没有就是 `None`。插件据此知道"哪个 agent 的键"。
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

/// [`AppKeyBindingsExt::add_key_binding`] 的观察者：按下时对显示中的 agent
/// 跑一条斜杠命令 `line`（不带 `/`）。上游 `keys.rs:455`。
///
/// 语义与上游一致：
/// * 只看按下（`released == false`）、且 [`KeyPressed::agent`] 有值；没有
///   `ShownAgent` 时什么也不做（不 panic）；
/// * 用 `run_chosen` 而不是 `RunCommand`：这条命令不是用户打的，被拒时
///   写一条错误 `Notice`（应用层的 notice 桥会显示），输入框保持原样——
///   上游对"键/列表选中的命令"就是这条规则（上游 `commands.rs` 的
///   `run_chosen`）；用户按 Enter 打的 `/…` 仍走 `RunCommand`。
pub fn run_command(
    line: impl Into<String>,
) -> impl FnMut(On<KeyPressed>, Commands) + Send + Sync + 'static {
    let line = line.into();
    move |pressed: On<KeyPressed>, mut commands: Commands| {
        if let (Some(agent), false) = (pressed.agent, pressed.released) {
            commands.queue(run_chosen(agent, line.clone()));
        }
    }
}

/// [`bind`] 拒绝绑定时的原因，供前端展示。
#[derive(Resource, Default, Debug)]
pub struct RefusedKeys(pub Vec<String>);

/// 内置动作，对齐上游 `InputAction`（0.47.0 的 32 个变体全在），另加
/// F10 的 [`InputAction::CopySelection`]。
///
/// 挂在 [`KeyBinding`] 实体上时，按键由视图自己执行；没有这个组件时触发
/// [`KeyPressed`]。插件也可以直接 spawn 一个带 `InputAction` 的绑定来重新
/// 绑定内置动作。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAction {
    /// 清空输入。
    Clear,
    /// 输入为空时退出（`editor` 里写 [`bevy::app::AppExit`]；非空时什么也不做）。
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
    /// 补全；列表关着时，turn 跑着就排队，否则停止浏览历史。
    Tab,
    /// 发送（Enter）。
    Send,
    /// 关补全；列表关着、turn 跑着时停 agent（`StopTurn`）。
    Stop,
    /// 粘贴图片；本 crate 还没有接图片剪贴板，先留空实现。
    PasteImage,
    /// 复制转写区里选中的内容（鼠标拖选或 Alt+↑/↓ 选中的条目）。
    ///
    /// 动作本身由 [`crate::clipboard::copy_selection_keys`] 读按键执行；
    /// 绑定只负责占住 `Ctrl+Shift+C`，并让 [`crate::editor::keys`] 把它当成
    /// 已消费的动作（输入框不会因此清空）。
    CopySelection,
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
/// 按源码逐条断言。除这 43 条外，[`add_built_in`] 还会生成 F10 的
/// `Ctrl+Shift+C`、转写区 Esc 与 Alt+1..9、Alt+0 共 12 条，一共 55 个内置
/// 绑定实体。
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

/// 内联回答键的标记：Alt+1..9（[`InputAction::AnswerInline`]）与 Alt+0
/// （[`InputAction::DismissInline`]）十条绑定带它。
///
/// [`arm_inline_keys`] 只动这些绑定，别的 Alt+数字绑定（插件的）不会被它
/// 开关。对应上游 `keys.rs` 的 `InlineKey`。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct InlineKey;

/// F10：`Ctrl+Shift+C` —— 复制转写区里选中的内容。
///
/// 键位先查过 [`built_in`]：`Ctrl+C` 是 [`InputAction::Clear`]，而字符键的
/// Shift 在 [`KeyPress::new`] 里会归一成字符本身（`Shift+c` 上报为
/// `Character("C")`）。所以这里必须写**大写** `C`：`Ctrl+Shift+C` 是
/// `ctrl+C`，`Ctrl+C` 是 `ctrl+c`，两个键不相等，不会和清空输入冲突（这也是
/// `workspace_column_bindings` 里 `alt+H`/`ctrl+alt+H` 的同一写法）。
///
/// 作用域是 [`KeyScope::Global`]：Alt+↑/↓ 选中条目后焦点在转写根上，Input
/// 作用域的绑定查不到。真正的复制由
/// [`crate::clipboard::copy_selection_keys`] 读按键完成；这个绑定只负责占住
/// 键、并把动作交给 `editor::keys` 消费（否则带修饰键的字符键会被当成普通
/// 输入或 Clear 处理）。
pub fn copy_selection_binding() -> KeyBinding {
    KeyBinding::new(
        KeyPress::new(character_key('C'), Modifiers::CONTROL | Modifiers::SHIFT),
        "Copy the selected transcript text",
    )
    .in_scope(KeyScope::Global)
}

/// 生成视图自带的内置绑定：43 条输入键、F10 的 `Ctrl+Shift+C`
/// （[`InputAction::CopySelection`]）、转写区 Esc，以及先挂 `Disabled` 的
/// 内联回答键（Alt+1..9、Alt+0）。
pub fn add_built_in(app: &mut App) {
    app.init_resource::<KeyFocus>();
    for (keys, label, action) in built_in() {
        let binding = KeyBinding::new(keys, label).in_scope(KeyScope::Input);
        if let Some(mut bound) = bind(app.world_mut(), binding) {
            bound.insert(action);
        }
    }
    // F10：Ctrl+Shift+C 复制转写选区。Global 作用域，选条目后也能收到；
    // 动作由 `clipboard::copy_selection_keys` 读同一批按键执行。
    if let Some(mut bound) = bind(app.world_mut(), copy_selection_binding()) {
        bound.insert(InputAction::CopySelection);
    }
    // 读转写时 Esc 把键交回输入框。
    let back = KeyBinding::new(KeyPress::key(Key::Escape), "Back to the input")
        .in_scope(KeyScope::Transcript);
    if let Some(mut bound) = bind(app.world_mut(), back) {
        bound.insert(InputAction::Deselect);
    }
    // 内联问题：Alt+1..9 选第 n 个，Alt+0 拒绝。没有显示中的问题时它们
    // 挂 `Disabled`，由 [`arm_inline_keys`] 按确认面板的内容开关。
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
            bound.insert((action, InlineKey, Disabled));
        }
    }
}

/// 横向滚动工作区的绑定：`Ctrl+Alt+H/L` 一屏、`Ctrl+Alt+Home/End` 到两端。
///
/// 键位理由（先查过 [`built_in`]）：
/// * `Alt+←/→` 是内置的 WordLeft/WordRight，同作用域冲突；
/// * `Ctrl+Alt+←/→` 没被内置表占用，但 picker 的 `picker_keys` 对方向键
///   **不看修饰键**（picker 打开时按 → 会展开选中项说明），会和滚动打架；
/// * `Ctrl+Alt+H/L` 是 niri 自己的 focus-column 约定；带 Ctrl/Alt 的字符键
///   picker 不处理（`typing == false`），编辑器也不会把它当输入；
/// * `Ctrl+Alt+Home/End` 单独绑成 Global 是为了遮住 Input 作用域的无修饰
///   回退：否则 `Ctrl+Alt+Home` 会退化成 `Home`，把光标移到行首。
///
/// 这些绑定**没有观察者**：滚动由 `crate::workspaces` 的 `scroll_workspace_keys`
/// 读同一批 [`KeyboardInput`] 处理（picker 打开时 `editor::keys` 不跑，绑定表
/// 也不分发）；绑定只负责占住键，避免回退到光标动作。
pub fn workspace_scroll_bindings() -> Vec<KeyBinding> {
    let ctrl_alt = Modifiers::CONTROL | Modifiers::ALT;
    let with = |key: Key, label: &str| KeyBinding::new(KeyPress::new(key, ctrl_alt), label);
    vec![
        with(character_key('h'), "Scroll the workspace left"),
        with(character_key('l'), "Scroll the workspace right"),
        with(Key::Home, "Scroll to the workspace's left end"),
        with(Key::End, "Scroll to the workspace's right end"),
    ]
}

/// 装上 [`workspace_scroll_bindings`]（幂等；同键已被别的全局绑定占用时
/// 只记 [`RefusedKeys`]，不 panic）。
pub fn add_workspace_scroll_keys(app: &mut App) {
    for binding in workspace_scroll_bindings() {
        let _ = bind(app.world_mut(), binding);
    }
}

/// 列宽预设 / 移动列的绑定（niri `switch-preset-column-width`、`move-column-left/right`）。
///
/// 键位理由（先查过 [`built_in`] 与 [`workspace_scroll_bindings`]）：
/// * `Ctrl+Alt+H/L` 已经是 L3 的滚动键，不能再给移动列；移动列是 niri 的
///   `Mod+Ctrl+H/L`，这里加上 Shift 变成 `Ctrl+Alt+Shift+H/L`——同一组方向键，
///   多按一个 Shift 就是"搬列"，和滚动只差一个修饰键。
/// * 改列宽用 `Alt+Shift+H/L`（左窄右宽），不带 Ctrl：它和滚动键、移动列键
///   都不重叠；`Alt+Shift` 也不是任何内置键（内置表里只有 Alt+字符与
///   Ctrl+Alt/Alt+方向键）。
/// * 字符键的 Shift 在 [`KeyPress::new`] 里会归一到字符本身（`Shift+h` 是
///   `Character("H")`），所以这里的 `H`/`L` 必须写**大写**；写小写会被归一成
///   `Ctrl+Alt+h`，和 L3 的滚动绑定冲突、被 `bind` 拒绝。
///
/// 这些绑定和 [`workspace_scroll_bindings`] 一样**没有观察者**：动作由
/// `crate::workspaces` 的 `column_workspace_keys` 读同一批 [`KeyboardInput`]
/// 处理（picker 打开时 `editor::keys` 不跑，绑定表也不分发），绑定只负责占住
/// 键、避免以后有插件把同键抢走。
pub fn workspace_column_bindings() -> Vec<KeyBinding> {
    let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
    let ctrl_alt_shift = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT;
    let with = |key: char, modifiers: Modifiers, label: &str| {
        KeyBinding::new(KeyPress::new(character_key(key), modifiers), label)
    };
    vec![
        with('H', alt_shift, "Shrink the active column"),
        with('L', alt_shift, "Widen the active column"),
        with('H', ctrl_alt_shift, "Move the active column left"),
        with('L', ctrl_alt_shift, "Move the active column right"),
    ]
}

/// 装上 [`workspace_column_bindings`]（幂等；同键已被占用时只记
/// [`RefusedKeys`]，不 panic）。
pub fn add_workspace_column_keys(app: &mut App) {
    for binding in workspace_column_bindings() {
        let _ = bind(app.world_mut(), binding);
    }
}

/// 这个 `Ask` 是否显示在**当前工作区**的确认面板里：没有 `ShownAgent`
/// （无视图的测试/导出路径）时全部可见；有 `ForAgent` 的只认当前 agent，
/// 无主的（app 级问题）也可见。
///
/// 和 `batch::confirm_panel_rows` 的过滤同一口径（那边的 `focused_visible`
/// 是私有函数）；[`arm_inline_keys`] 与键盘作答都用它挑"当前确认 tile 的
/// 问题"。
pub fn ask_is_visible(shown: Option<Entity>, owner: Option<Entity>) -> bool {
    shown.is_none_or(|shown| owner.is_none_or(|owner| owner == shown))
}

/// 确认面板里有未回答的 `Ask` 时启用 Alt+1..9 / Alt+0，没有时挂回
/// `Disabled`（上游 `keys.rs::arm_inline_keys`）。
///
/// 只在"有问题显示"时占住这些键，所以没有问题时 Alt+1..9 会落回插件或
/// 输入框，不会被内联绑定吞掉。启不启用看的是显示中的问题，不是本帧的
/// 按键；Commands 在本系统与 [`crate::editor::keys`] 之间自动同步，所以问题
/// 出现/答完后的下一次检查就生效（问题由内核在 AgentPass 里 spawn 时最多
/// 晚一帧，和上游按帧更新一致）。
pub fn arm_inline_keys(
    asks: Query<Option<&ForAgent>, (With<Ask>, Without<Outcome>)>,
    shown: Option<Res<ShownAgent>>,
    keys: Query<(Entity, Has<Disabled>), (With<InlineKey>, Allow<Disabled>)>,
    mut commands: Commands,
) {
    let shown = shown.as_deref().and_then(|shown| shown.0);
    let showing = asks
        .iter()
        .any(|owner| ask_is_visible(shown, owner.map(|ForAgent(owner)| *owner)));
    for (key, disabled) in &keys {
        match (showing, disabled) {
            (true, true) => {
                commands.entity(key).remove::<Disabled>();
            }
            (false, false) => {
                commands.entity(key).insert(Disabled);
            }
            _ => {}
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
    fn the_copy_selection_binding_is_ctrl_shift_c_and_does_not_collide_with_clear() {
        let copy = copy_selection_binding();
        assert_eq!(copy.keys.to_string(), "ctrl+C", "Shift 归一到字符 C");
        assert_eq!(copy.scope, KeyScope::Global, "选条目后焦点在转写根上");
        for (keys, label, action) in built_in() {
            let other = KeyBinding::new(keys, label).in_scope(KeyScope::Input);
            assert!(
                !copy.conflicts(&other),
                "`{}` 不能和内置 `{}`（{action:?}）冲突",
                copy.keys,
                other.keys
            );
        }
        // Ctrl+C（Clear）与 Ctrl+Shift+C 是两个键：前者小写 c，后者大写 C。
        let clear = KeyBinding::new(KeyPress::ctrl('c'), "Clear").in_scope(KeyScope::Input);
        assert!(!copy.conflicts(&clear));
        assert_ne!(copy.keys, clear.keys);
        // 真实按下去（窗口上报 Character("C") + Ctrl/Shift）要命中 CopySelection，
        // 输入框（Input 作用域）和转写根（Panel 作用域）都一样；Ctrl+C 仍是 Clear。
        let mut app = App::new();
        add_built_in(&mut app);
        let mut query =
            app.world_mut()
                .query::<(Entity, &KeyBinding, Option<&InputAction>)>();
        let collected: Vec<(Entity, &KeyBinding, Option<InputAction>)> = query
            .iter(app.world())
            .map(|(entity, binding, action)| (entity, binding, action.copied()))
            .collect();
        let rows: Vec<BindingRef> = collected
            .iter()
            .map(|(entity, binding, action)| BindingRef {
                entity: *entity,
                binding,
                action: *action,
            })
            .collect();
        let copy_press = KeyPress::new(
            Key::Character("C".into()),
            Modifiers::CONTROL | Modifiers::SHIFT,
        );
        let panel = Entity::from_raw_u32(77).unwrap();
        for scope in [KeyScope::Input, KeyScope::Panel(panel)] {
            assert_eq!(
                action_for(&rows, std::slice::from_ref(&copy_press), scope),
                Some(InputAction::CopySelection),
                "Ctrl+Shift+C 在 {scope:?} 里要命中复制"
            );
        }
        assert_eq!(
            action_for(&rows, &[KeyPress::ctrl('c')], KeyScope::Input),
            Some(InputAction::Clear),
            "Ctrl+C 在输入框里仍是 Clear"
        );
    }

    #[test]
    fn workspace_scroll_keys_are_free_and_shadow_the_bare_home_and_end() {
        let scroll = workspace_scroll_bindings();
        assert_eq!(scroll.len(), 4, "H/L + Home/End");
        let built: Vec<KeyBinding> = built_in()
            .into_iter()
            .map(|(keys, label, _)| KeyBinding::new(keys, label).in_scope(KeyScope::Input))
            .collect();
        for binding in &scroll {
            for other in &built {
                assert!(
                    !binding.conflicts(other),
                    "`{}` 不能和内置 `{}`（{}）冲突",
                    binding.keys,
                    other.keys,
                    other.label
                );
            }
        }
        for binding in &scroll {
            assert_eq!(binding.scope, KeyScope::Global);
        }

        // Global 的 Ctrl+Alt+Home 要遮住 Input 作用域里的 Home（否则编辑器
        // 会把 Ctrl+Alt+Home 当成 Home，把光标移到行首）。
        let mut app = App::new();
        add_built_in(&mut app);
        add_workspace_scroll_keys(&mut app);
        let mut query = app
            .world_mut()
            .query::<(Entity, &KeyBinding, Option<&InputAction>)>();
        let collected: Vec<(Entity, &KeyBinding, Option<InputAction>)> = query
            .iter(app.world())
            .map(|(entity, binding, action)| (entity, binding, action.copied()))
            .collect();
        let rows: Vec<BindingRef> = collected
            .iter()
            .map(|(entity, binding, action)| BindingRef {
                entity: *entity,
                binding,
                action: *action,
            })
            .collect();
        let press = KeyPress::new(Key::Home, Modifiers::CONTROL | Modifiers::ALT);
        let found = find(&rows, std::slice::from_ref(&press), KeyScope::Input);
        let entity = match found {
            Found::Binding(entity) => entity,
            other => panic!("Ctrl+Alt+Home 应该命中我们的绑定，实际 {other:?}"),
        };
        let row = rows
            .iter()
            .find(|row| row.entity == entity)
            .expect("命中的绑定");
        assert_eq!(row.binding.keys.to_string(), "ctrl+alt+home");
        assert!(row.action.is_none(), "键由 workspaces 读原始事件，不挂动作");
    }

    #[test]
    fn workspace_column_keys_are_free_of_built_in_and_scroll_bindings() {
        let column = workspace_column_bindings();
        assert_eq!(column.len(), 4, "改宽 H/L + 移动列 H/L");
        let scroll = workspace_scroll_bindings();
        let built: Vec<KeyBinding> = built_in()
            .into_iter()
            .map(|(keys, label, _)| KeyBinding::new(keys, label).in_scope(KeyScope::Input))
            .collect();
        for binding in &column {
            assert_eq!(binding.scope, KeyScope::Global);
            for other in scroll.iter().chain(built.iter()) {
                assert!(
                    !binding.conflicts(other),
                    "`{}` 不能和 `{}`（{}）冲突",
                    binding.keys,
                    other.keys,
                    other.label
                );
            }
        }
        // 字符键的 Shift 由字符本身携带：绑定是 `alt+H` / `ctrl+alt+H`
        // （不是 `alt+shift+h`），这样才和窗口上报的 `Shift+h → H` 对得上，
        // 也不会和 L3 的小写 `ctrl+alt+h` 滚动键撞。
        let keys: Vec<String> = column.iter().map(|binding| binding.keys.to_string()).collect();
        assert_eq!(
            keys,
            vec!["alt+H", "alt+L", "ctrl+alt+H", "ctrl+alt+L"]
        );

        // 绑定实体能全部装上（不和内置键/滚动键冲突，也没有被拒绝）。
        let mut app = App::new();
        add_built_in(&mut app);
        add_workspace_scroll_keys(&mut app);
        add_workspace_column_keys(&mut app);
        let refused = app
            .world()
            .get_resource::<RefusedKeys>()
            .map(|refused| refused.0.clone())
            .unwrap_or_default();
        assert!(refused.is_empty(), "列宽/移动列绑定不能被拒绝：{refused:?}");
    }

    #[test]
    fn add_built_in_spawns_43_plus_copy_plus_transcript_esc_plus_10_inline() {
        let mut app = App::new();
        add_built_in(&mut app);
        // 默认查询会跳过 Disabled，所以显式 Allow 它们，数全部内置实体。
        let count = app
            .world_mut()
            .query_filtered::<&KeyBinding, Allow<Disabled>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 43 + 1 + 1 + 10, "43 输入 + Ctrl+Shift+C + Esc + 10 内联");
        let enabled = app
            .world_mut()
            .query::<&KeyBinding>()
            .iter(app.world())
            .count();
        assert_eq!(enabled, 43 + 1 + 1, "Alt+0..9 先 Disabled");
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

    // ---- run_command / KeyPressed.agent：键 -> ShownAgent -> 斜杠命令 ----

    use rig_harness::commands::{AppCommandsExt, CommandArgs};
    use rig_ecs::prelude::{Notice, NoticeLevel};

    /// 无窗口的最小 App：装了 `editor::keys`，所以插件绑定会真的收到
    /// `KeyPressed`。`shown` 为 `None` 时**连 `ShownAgent` 资源都不插**，
    /// 覆盖"没有显示中的 agent"的路径。
    fn key_app(shown: Option<Entity>) -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<crate::editor::PromptSubmitted>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<crate::editor::Editor>()
            .init_resource::<crate::editor::SubmittedPrompts>()
            .init_resource::<KeyFocus>()
            .init_resource::<PendingInput>();
        add_built_in(&mut app);
        app.add_systems(Update, (arm_inline_keys, crate::editor::keys).chain());
        if let Some(agent) = shown {
            app.insert_resource(ShownAgent(Some(agent)));
        }
        app
    }

    fn send(app: &mut App, key: Key, code: KeyCode) {
        app.world_mut().write_message(press_event(key, code));
        app.update();
    }

    /// Ctrl+G：先按 Ctrl（`editor::keys` 从 `ButtonInput` 读修饰键），再发 G，
    /// 最后松开。
    fn press_ctrl_g(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        send(app, Key::Character("g".into()), KeyCode::KeyG);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ControlLeft);
    }

    /// 一条命令跑过时的 `(agent, 参数)`。
    #[derive(Resource, Default)]
    struct Ran(Vec<(Entity, String)>);

    fn probe_command(In(args): In<CommandArgs>, mut ran: ResMut<Ran>) {
        ran.0.push((args.agent, args.args.clone()));
    }

    #[test]
    fn run_command_runs_the_line_for_the_shown_agent() {
        let mut app = key_app(None);
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)))
            .init_resource::<Ran>()
            .add_command("probe", "Probe", probe_command);
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('g'), "Probe").in_scope(KeyScope::Global),
                run_command("probe hello"),
            )
            .is_some()
        );

        press_ctrl_g(&mut app);
        assert_eq!(
            app.world().resource::<Ran>().0,
            [(agent, "hello".to_owned())],
            "按键应把 `probe hello` 跑在 ShownAgent 上"
        );
        assert_eq!(
            app.world().resource::<crate::editor::Editor>().text(),
            "",
            "键跑的斜杠命令不碰输入框"
        );
    }

    #[test]
    fn run_command_does_nothing_without_a_shown_agent() {
        let mut app = key_app(None);
        app.init_resource::<Ran>()
            .add_command("probe", "Probe", probe_command);
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('g'), "Probe").in_scope(KeyScope::Global),
                run_command("probe hello"),
            )
            .is_some()
        );

        press_ctrl_g(&mut app);
        assert!(
            app.world().resource::<Ran>().0.is_empty(),
            "没有 ShownAgent：不 panic，也不跑命令"
        );
    }

    #[test]
    fn a_refused_run_command_is_an_error_notice_for_the_agent() {
        let mut app = key_app(None);
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)))
            .add_message::<Notice>()
            .add_command("probe", "Probe", probe_command);
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('g'), "Missing").in_scope(KeyScope::Global),
                run_command("missing"),
            )
            .is_some()
        );

        press_ctrl_g(&mut app);
        let notices: Vec<Notice> = app
            .world()
            .resource::<Messages<Notice>>()
            .iter_current_update_messages()
            .cloned()
            .collect();
        assert_eq!(notices.len(), 1, "被拒的命令写一条 Notice");
        assert_eq!(notices[0].agent, Some(agent), "Notice 是关于 ShownAgent 的");
        assert_eq!(notices[0].level, NoticeLevel::Error);
        assert!(notices[0].text.contains("/missing"), "{}", notices[0].text);
        assert_eq!(
            app.world().resource::<crate::editor::Editor>().text(),
            "",
            "键跑的命令被拒也不回填输入"
        );
    }

    /// 收到 `KeyPressed.agent` 的观察者。
    #[derive(Resource, Default)]
    struct SeenAgents(Vec<Option<Entity>>);

    #[test]
    fn key_pressed_agent_is_the_shown_agent_and_follows_a_switch() {
        let mut app = key_app(None);
        let first = app.world_mut().spawn(Name::new("first")).id();
        let second = app.world_mut().spawn(Name::new("second")).id();
        app.insert_resource(ShownAgent(Some(first)))
            .init_resource::<SeenAgents>()
            .add_observer(|pressed: On<KeyPressed>, mut seen: ResMut<SeenAgents>| {
                seen.0.push(pressed.agent);
            });
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('g'), "Mark").in_scope(KeyScope::Global),
                |_: On<KeyPressed>| {},
            )
            .is_some()
        );

        press_ctrl_g(&mut app);
        assert_eq!(app.world().resource::<SeenAgents>().0, [Some(first)]);

        // 切 agent：同一个绑定下一次拿到新的 ShownAgent。
        app.world_mut().resource_mut::<ShownAgent>().0 = Some(second);
        press_ctrl_g(&mut app);
        assert_eq!(
            app.world().resource::<SeenAgents>().0,
            [Some(first), Some(second)]
        );

        // 没有显示中的 agent：字段是 None，观察者照常收到。
        app.world_mut().resource_mut::<ShownAgent>().0 = None;
        press_ctrl_g(&mut app);
        assert_eq!(
            app.world().resource::<SeenAgents>().0,
            [Some(first), Some(second), None]
        );
    }

    #[test]
    fn key_pressed_agent_is_also_set_when_the_key_is_released() {
        /// 松开时收到的 `(agent, released)`。
        #[derive(Resource, Default)]
        struct Seen(Vec<(Option<Entity>, bool)>);

        let mut app = key_app(None);
        let agent = app.world_mut().spawn(Name::new("agent")).id();
        app.insert_resource(ShownAgent(Some(agent)))
            .init_resource::<Seen>()
            .add_observer(|pressed: On<KeyPressed>, mut seen: ResMut<Seen>| {
                seen.0.push((pressed.agent, pressed.released));
            });
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('h'), "Hold")
                    .in_scope(KeyScope::Global)
                    .with_releases(),
                |_: On<KeyPressed>| {},
            )
            .is_some()
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyH,
            logical_key: Key::Character("h".into()),
            state: ButtonState::Released,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(
            app.world().resource::<Seen>().0,
            [(Some(agent), true)],
            "松开也带 ShownAgent，released 为 true"
        );
    }

    #[test]
    fn key_pressed_agent_is_none_without_the_shown_agent_resource() {
        let mut app = key_app(None);
        app.init_resource::<SeenAgents>()
            .add_observer(|pressed: On<KeyPressed>, mut seen: ResMut<SeenAgents>| {
                seen.0.push(pressed.agent);
            });
        assert!(
            app.add_key_binding(
                KeyBinding::new(KeyPress::ctrl('g'), "Mark").in_scope(KeyScope::Global),
                |_: On<KeyPressed>| {},
            )
            .is_some()
        );

        press_ctrl_g(&mut app);
        assert_eq!(app.world().resource::<SeenAgents>().0, [None]);
    }
}
