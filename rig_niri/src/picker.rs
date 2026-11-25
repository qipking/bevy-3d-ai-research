//! 列表选择器（picker）：rig 的插件用 `Picker` 让用户从列表里挑（会话、模型、命令…）。
//!
//! rig-tui 把 picker 画成**盖住整屏的浮层**：标题、说明、输入行、列表、底部键位行，
//! 选中项高亮，回车 `Pick::person(picker, Some(Chosen::item(i)))`，Esc 关闭；
//! 打字过滤列表（每个词都要出现，标签命中的排在前面），`free_text` 的 picker
//! 还能把打的字本身当答案；`multiple` 的 picker 用 Tab 打勾、回车交 `Chosen::items`；
//! 有 `PickItem::parent` 时是一棵树，←/→ 折叠。
//!
//! 键位和上游 `pick::choice_key` 对齐的部分：Alt+1..9 直接选并确认第 n 个可见项
//! （Alt+0 取消，和 Esc 一样），但只在 picker 显示时由 [`picker_keys`] 接管；没有
//! picker 时这些键仍归输入框（Ask 的 [`crate::keys::InlineKey`]）/ 插件绑定。
//! 非树 picker 的 → 展开选中项的完整说明（`PickItem::more`，没有就用 `detail`；
//! 再按一次收起，← 也收起），↑↓/PgUp/PgDn 移动时列表体跟着滚动；过滤/重新填充
//! 后按条目身份（标签，其次条目下标）把选中项找回来。
//!
//! **这里没有浮层**：picker 是条带最右边的一列——在 `FrameTree` 根的
//! `Split::Horizontal` 末尾追加一个 `Child`，里面是一块 [`PanelId::Picker`]
//! 容器（底色 + 边框），容器内部再竖着分成 [`PanelId::List`]（标题 / 说明 /
//! 过滤 / 列表 / 键位行）和可选的 [`PanelId::Preview`]。关掉时把这一列摘掉、
//! despawn 三块面板、清掉行槽。这就是 niri 的"新窗口 = 右边新加一列"。
//!
//! 同时有多个 picker 时只有一个列：它们按 [`PickerQueue`] 里的创建顺序排队，
//! 一次显示队首，关掉（`Pick` 之后 harness despawn）或被外部回收后队首接上，
//! 队列空了才把列摘掉——和上游 `pick::queue_choices` 的顺序一致。`Ask` 不进
//! 这个队列：它是条带里的确认 tile（见 `batch::confirm_rows`），没有弹层。

use bevy::prelude::*;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::{ButtonInput, ButtonState};
use rig_harness::pick::{PickItem, PickPreview, TickBox, Ticked};
use rig_harness::prelude::{Chosen, Highlighted, Pick, Picker};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{FramePanel, FrameTree, Look, PanelAreas, PanelRoots, spawn_panel};
use crate::keys::Modifiers;
use crate::layout::{Child, Node, NO_GAP, Size, Split};
use crate::panels::{KeyedRow, PanelId, PanelRows, PanelSpawned};
use crate::theme::Theme;
use crate::transcript::Row;
use crate::workspaces::{Workspace, WorkspaceSet};
use crate::FeathersSystems;

/// rig-tui 的 `PAGE`：PgUp/PgDn 一次跳几个。
const PAGE: usize = 10;

/// 一行就是一个选项：点它回答这个 picker。
#[derive(Component, Clone)]
pub struct PickerChoice {
    pub picker: Entity,
    pub index: usize,
}

/// 打开着的 picker 的状态（rig-tui `PickerInput` 的收窄版）。
#[derive(Resource, Default)]
pub struct PickerInput {
    /// 打开的是哪个 picker。
    pub picker: Option<Entity>,
    /// picker 那一列的容器面板（[`PanelId::Picker`]：底色 + 边框，本身不产行）。
    pub column: Option<Entity>,
    /// 列里的列表 tile（[`PanelId::List`]，picker 的所有行都写在这里）。
    pub list_panel: Option<Entity>,
    /// 列里的 preview tile（[`PanelId::Preview`]，`PickPreview` 在时才有）。
    pub preview_panel: Option<Entity>,
    /// 打进去的过滤词。
    pub filter: String,
    /// 树上展开的分支（rig 的 `open`）。
    pub open: Vec<usize>,
    /// 打勾的条目（rig 的 `ticked`）。
    pub ticked: Vec<usize>,
    /// 用户动过勾（rig 的 `touched`）：多选时回车交的是勾上的条目。
    pub touched: bool,
    /// 过滤 + 折叠后可见的行：`Some(item)` 是条目，`None` 是 free_text 的作答行。
    pub rows: Vec<Option<usize>>,
    /// 选中的是第几行。
    pub selected: usize,
    /// 列表体的第一行（rig 的 `scroll`）：选中项永远落在可见窗口里，
    /// 渲染时按面板能放的行数把窗口滚过去。
    pub scroll: usize,
    /// 是否展开选中项的完整说明（rig 的 `more`）：展开的说明行紧跟在选中行
    /// 下面，再按一次收起。
    pub more: bool,
    /// 上一次选中的条目身份 `(条目下标, 标签)`（rig 的 `picked`）：过滤词
    /// 变化或插件重新填充条目后，按它把选中项找回来。
    pub picked: Option<(usize, String)>,
}

impl PickerInput {
    pub fn is_open(&self) -> bool {
        self.picker.is_some()
    }
}

/// 打开着的 picker 的显示顺序（rig-tui `TuiView::queue` 的收窄版）：一次只显示
/// 一个，关掉队首后下一个接上。
///
/// 入队发生在 `Picker` 组件真正加进 world 的那一刻：[`PickerPlugin`] 的
/// `Add<Picker>` 观察者按 spawn / insert 的先后同步执行，所以队列顺序就是
/// **创建顺序**，和 ECS 查询的迭代顺序无关（despawn 的 swap-remove 会打乱
/// 后者）。[`sync_picker`] 每帧把查询里漏掉的 picker 追加到队尾兜底，并清掉
/// 已经 despawn 的条目；[`PickerQueue::push`] 对同一实体去重，所以同一个
/// picker 不会因为重复的 add/spawn 事件被排两次。
#[derive(Resource, Default, Debug)]
pub struct PickerQueue {
    queue: Vec<Entity>,
}

impl PickerQueue {
    /// 把 `picker` 放到队尾；已经在队里就什么都不做。
    pub fn push(&mut self, picker: Entity) {
        if !self.queue.contains(&picker) {
            self.queue.push(picker);
        }
    }

    /// 队首：正在显示（或下一个要显示）的 picker。
    pub fn front(&self) -> Option<Entity> {
        self.queue.first().copied()
    }

    /// 排队中的 picker 顺序（测试与调试用）。
    pub fn entities(&self) -> &[Entity] {
        &self.queue
    }

    /// 队列里有几个 picker（测试与调试用）。
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是不是空的（测试与调试用）。
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 只留下 `alive` 说还在的：关闭 / 被外部 despawn 的 picker 出队。
    fn retain(&mut self, mut alive: impl FnMut(Entity) -> bool) {
        self.queue.retain(|entity| alive(*entity));
    }
}

/// 没有 picker 打开：输入框和转写窗格才吃按键（rig-tui 用 `KeyScope` 做同一件事）。
pub fn no_picker(pickers: Query<&Picker>) -> bool {
    pickers.is_empty()
}

// ---------------------------------------------------------------------------
// picker 的纯逻辑（rig-tui `Listing` 的同名规则的收窄版）
// ---------------------------------------------------------------------------

/// 一个词都要出现才算命中（rig-tui `pick::holds`）。
fn holds(lower: &str, said: &[Option<&str>]) -> bool {
    let said = said.iter().flatten().copied().collect::<Vec<_>>().join("\n");
    let said = said.to_lowercase();
    lower.split_whitespace().all(|word| said.contains(word))
}

/// `index` 坐在哪个分支下（只在分支列在它前面时算）。
fn parent(picker: &Picker, index: usize) -> Option<usize> {
    let parent = picker.items.get(index)?.parent?;
    (parent < index).then_some(parent)
}

/// `index` 上面所有的分支，最近的在前。
fn branches(picker: &Picker, index: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(parent(picker, index), move |branch| parent(picker, *branch))
}

/// 有没有树（有条目坐在别的条目下面）。
fn tree(picker: &Picker) -> bool {
    picker.items.iter().any(|item| item.parent.is_some())
}

/// `index` 是不是分支（有东西坐在它下面）。
fn is_branch(picker: &Picker, index: usize) -> bool {
    (0..picker.items.len()).any(|other| parent(picker, other) == Some(index))
}

/// 树里这一帧显示哪些条目（rig-tui `Listing::shown`）。
fn shown(picker: &Picker, open: &[usize], finds: Option<&dyn Fn(&PickItem) -> bool>) -> Vec<bool> {
    let Some(finds) = finds else {
        return (0..picker.items.len())
            .map(|index| branches(picker, index).all(|branch| open.contains(&branch)))
            .collect();
    };
    let found: Vec<bool> = picker.items.iter().map(|item| finds(item)).collect();
    let mut shown = found.clone();
    for index in (0..picker.items.len()).rev() {
        if shown.get(index) == Some(&true)
            && let Some(above) = parent(picker, index)
            && let Some(slot) = shown.get_mut(above)
        {
            *slot = true;
        }
    }
    for (index, slot) in shown.iter_mut().enumerate() {
        let mut above = branches(picker, index);
        let through = above
            .find(|branch| !open.contains(branch) || found.get(*branch) == Some(&true))
            .is_some_and(|branch| open.contains(&branch));
        *slot |= through;
    }
    shown
}

/// 过滤 + 折叠后的行：树里是"分支打开的那些"，平表里标签命中的在前。
fn rows_for(picker: &Picker, filter: &str, open: &[usize]) -> Vec<Option<usize>> {
    let lower = filter.to_lowercase();
    let finds = |item: &PickItem| {
        holds(
            &lower,
            &[Some(item.label.as_str()), item.detail.as_deref()],
        )
    };
    let mut rows: Vec<Option<usize>> = if tree(picker) {
        let typed = !lower.trim().is_empty();
        let shown = shown(picker, open, typed.then_some(&finds as &dyn Fn(&PickItem) -> bool));
        shown
            .iter()
            .enumerate()
            .filter(|(_, shown)| **shown)
            .map(|(index, _)| Some(index))
            .collect()
    } else {
        let found: Vec<(usize, &PickItem)> = picker
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| finds(item))
            .collect();
        let (named, beside): (Vec<_>, Vec<_>) = found.into_iter().partition(|(_, item)| {
            holds(&lower, &[Some(item.label.as_str())])
        });
        named
            .into_iter()
            .chain(beside)
            .map(|(index, _)| Some(index))
            .collect()
    };
    if picker.free_text {
        rows.push(None);
    }
    rows
}

/// 条目 `index` 的勾选框（rig-tui `TickBox::of`）。
fn tick_box(picker: &Picker, index: usize, ticked: &[usize]) -> TickBox {
    let parents: Vec<Option<usize>> = picker.items.iter().map(|item| item.parent).collect();
    TickBox::of(&parents, index, ticked)
}

/// 条目 `index` 展开时显示的完整说明：`PickItem::more`，没有就用 `detail`
/// （和上游 `Listing` 把 `more` 退回 `detail` 同一条规则）。
fn more_text<'a>(picker: &'a Picker, index: usize) -> Option<&'a str> {
    let item = picker.items.get(index)?;
    item.more.as_deref().or(item.detail.as_deref())
}

/// 条目 `index` 的身份 `(条目下标, 标签)`（rig 的 `picked`）。
fn identity(picker: &Picker, index: usize) -> Option<(usize, String)> {
    picker
        .items
        .get(index)
        .map(|item| (index, item.label.clone()))
}

/// 当前选中行的条目身份；选中 free_text 的作答行时没有身份。
fn identity_of(state: &PickerInput, picker: &Picker) -> Option<(usize, String)> {
    identity(picker, state.rows.get(state.selected).copied().flatten()?)
}

/// 在 `rows` 里找回 `picked`：先找同标签的条目，再找同一条目下标的
/// （过滤只留下序号对不上但身份相同的情况；两个都没有就是 `None`）。
fn find_picked(
    rows: &[Option<usize>],
    picker: &Picker,
    picked: Option<&(usize, String)>,
) -> Option<usize> {
    let (index, label) = picked?;
    rows.iter()
        .position(|row| {
            row.and_then(|at| picker.items.get(at))
                .is_some_and(|item| item.label == *label)
        })
        .or_else(|| rows.iter().position(|row| *row == Some(*index)))
}

/// 展开的说明行：`text` 按 `width` 列折行（和 body 用同一个 `wrap_all`），每行
/// 缩进 4 格、宽度里已经扣掉这 4 格，所以任何一行的显示宽度都不超过列宽。
fn more_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    // 窄面板上缩进也跟着缩，保证"缩进 + 折行宽度"不超过列宽。
    let indent = width.saturating_sub(1).min(4);
    rig_tui::wrap::wrap_all(&lines, width.saturating_sub(indent).max(1))
        .into_iter()
        .map(|line| {
            let line = line.patch_style(style);
            let mut spans = vec![span(" ".repeat(indent), style)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// 重算可见行，并按身份把选中项找回来（rig 的 `refill`）。
///
/// 插件重新填充条目、树展开/折叠、picker 刚打开时都走这里：同标签的条目
/// 优先，其次是同一条目下标；都没有就留在原来那行（夹回范围）。
fn refill(state: &mut PickerInput, picker: &Picker) {
    // `picked` 优先：插件重新填充时 `state.rows` 还是旧条目的行号，按它算身份
    // 会把新条目的同一行号错认成选中项。
    let picked = state.picked.clone().or_else(|| identity_of(state, picker));
    state.rows = rows_for(picker, &state.filter, &state.open);
    state.selected = find_picked(&state.rows, picker, picked.as_ref())
        .unwrap_or(state.selected)
        .min(state.rows.len().saturating_sub(1));
    state.picked = identity_of(state, picker);
    state.scroll = state.scroll.min(state.selected);
}

/// 过滤词变了：重算可见行，选中的条目还在列表里就留在它上面，不在了回到
/// 第一行（和上游 `found_row` 的“找不到就 0”一致；`picked` 让同一个标签
/// 在过滤后仍被选中）。
fn refilter(state: &mut PickerInput, picker: &Picker) {
    let picked = state.picked.clone().or_else(|| identity_of(state, picker));
    state.rows = rows_for(picker, &state.filter, &state.open);
    state.selected = find_picked(&state.rows, picker, picked.as_ref())
        .unwrap_or(0)
        .min(state.rows.len().saturating_sub(1));
    state.picked = identity_of(state, picker);
    state.scroll = state.scroll.min(state.selected);
}

/// 让选中行（以及它下面 `tail` 行，比如刚展开的说明）落进 `visible` 行的
/// 窗口里，返回窗口第一行（rig 的 `scroll`，记的是列表体的第一行）。
///
/// 第一行不会越过选中行本身：滚动只跟着选中项走，不会把选中项滚出视野；
/// 首尾都夹在 `0..=总行数-可见行数` 里，选项不够一屏时窗口就是 0。
fn scroll_to_selected(state: &mut PickerInput, visible: usize, total: usize, tail: usize) -> usize {
    let visible = visible.max(1);
    let end = state.selected.saturating_add(1 + tail);
    if state.selected < state.scroll {
        state.scroll = state.selected;
    }
    if end > state.scroll.saturating_add(visible) {
        state.scroll = end.saturating_sub(visible);
    }
    state.scroll = state
        .scroll
        .min(state.selected)
        .min(total.saturating_sub(visible));
    state.scroll
}

/// 树上 ←/→：打开分支、落到第一个子项，或者关上、回到父项（rig 的 `unfold`）。
fn unfold(state: &mut PickerInput, picker: &Picker, open: bool) {
    let Some(Some(index)) = state.rows.get(state.selected).copied() else {
        return;
    };
    let is_open = state.open.contains(&index);
    match (open, is_branch(picker, index), is_open) {
        (true, true, false) => {
            state.open.push(index);
            refill(state, picker);
        }
        (true, true, true) => {
            if let Some(Some(next)) = state.rows.get(state.selected + 1).copied()
                && branches(picker, next).any(|branch| branch == index)
            {
                state.selected += 1;
            }
        }
        (false, true, true) => {
            state.open.retain(|branch| *branch != index);
            refill(state, picker);
        }
        (false, _, _) => {
            if let Some(up) = parent(picker, index)
                && let Some(at) = state.rows.iter().position(|row| *row == Some(up))
            {
                state.selected = at;
            }
        }
        (true, false, _) => {}
    }
}

/// Tab：给选中项打勾/取消。
///
/// rig 的分支勾是两段式（第一次只勾中分支、第二次才勾叶子，所以分支会先显示
/// `Off`）；3D 这边做成**一次整枝切换**：勾上就勾整枝，取消就整枝取消。
/// 显示仍然用 rig 的 `TickBox::of` 算（`[x]` / `[-]` / `[ ]`）。
fn tick(state: &mut PickerInput, picker: &Picker) {
    let Some(Some(index)) = state.rows.get(state.selected).copied() else {
        return;
    };
    if is_branch(picker, index) {
        let under: Vec<usize> = (0..picker.items.len())
            .filter(|item| *item == index || branches(picker, *item).any(|branch| branch == index))
            .collect();
        match tick_box(picker, index, &state.ticked) {
            TickBox::Off => state.ticked.extend(under),
            TickBox::On | TickBox::Mixed => state.ticked.retain(|ticked| !under.contains(ticked)),
        }
        state.ticked.sort_unstable();
        state.ticked.dedup();
    } else {
        match state.ticked.binary_search(&index) {
            Ok(at) => {
                state.ticked.remove(at);
            }
            Err(at) => state.ticked.insert(at, index),
        }
    }
    state.touched = true;
}

/// 回车交什么（rig 的 `answer`）：选中项，或者动过勾的多选交勾上的；填空的交文字。
fn answer(state: &PickerInput, picker: &Picker) -> Option<Chosen> {
    match state.rows.get(state.selected).copied().flatten() {
        Some(_) if state.touched && picker.multiple => Some(Chosen::items(state.ticked.clone())),
        Some(index) => Some(Chosen::item(index)),
        None if !picker.free_text => None,
        None if state.filter.trim().is_empty() => None,
        None => Some(Chosen::items(state.ticked.clone()).with_text(state.filter.trim())),
    }
}

// ---------------------------------------------------------------------------
// 系统
// ---------------------------------------------------------------------------

fn key_of(parts: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

fn span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// picker 那一列的树节点：容器 [`PanelId::Picker`] 里竖着排列表 / preview。
///
/// 没有 preview 时容器里只有一块 [`PanelId::List`]；有 preview 时是一列内部的
/// 竖直分裂（`List` 2 份、`Preview` 1 份，`NO_GAP`——两个 tile 自己带边框）。
fn picker_column(preview: bool) -> Node {
    let inner = if preview {
        Node::split(
            Split::Vertical,
            NO_GAP,
            vec![
                Child::proportion(2.0, Node::leaf(PanelId::List)),
                Child::proportion(1.0, Node::leaf(PanelId::Preview)),
            ],
        )
    } else {
        Node::leaf(PanelId::List)
    };
    Node::container(PanelId::Picker, inner)
}

/// 在聚焦工作区的树（有 [`WorkspaceSet`] 时）和全局 [`FrameTree`] 上做同一件事。
///
/// 两处都要改：布局对聚焦工作区读全局 `FrameTree`（见 `frame::lay_out_frame`），
/// 而 `commit_focused_panels` 在 `Render` 把全局树写回 `Workspace.tree`；直接
/// 改组件能让"只跑 picker、不跑 PanelsPlugin"的 App 也看到同一棵树。
fn edit_trees(
    tree: &mut Option<ResMut<FrameTree>>,
    set: Option<&WorkspaceSet>,
    workspaces: &mut Query<'_, '_, &mut Workspace>,
    mut edit: impl FnMut(&mut Node) -> bool,
) -> bool {
    let mut changed = false;
    if let Some(set) = set
        && let Some(focused) = set.focused
        && let Ok(mut workspace) = workspaces.get_mut(focused)
    {
        changed |= edit(&mut workspace.tree);
    }
    if let Some(tree) = tree.as_deref_mut() {
        changed |= edit(&mut tree.0);
    }
    changed
}

/// 把 picker 列从每一棵工作区的树里摘掉（关闭时用，防聚焦切换时留下残叶）。
fn remove_columns_everywhere(
    tree: &mut Option<ResMut<FrameTree>>,
    set: Option<&WorkspaceSet>,
    workspaces: &mut Query<'_, '_, &mut Workspace>,
) {
    if let Some(set) = set {
        for entity in set.items.iter().copied() {
            if let Ok(mut workspace) = workspaces.get_mut(entity) {
                workspace.tree.remove_column(PanelId::List);
            }
        }
    }
    if let Some(tree) = tree.as_deref_mut() {
        tree.0.remove_column(PanelId::List);
    }
}

/// 把 preview 叶子从**每一棵**工作区的树里摘掉（关闭时用）。
///
/// 不能只看聚焦工作区：picker 列可能开在之前聚焦的那棵树上，切过工作区后
/// 旧树的 preview 叶子也要清，否则摘掉列表列之后会剩下一块空 preview 列。
fn remove_previews_everywhere(
    tree: &mut Option<ResMut<FrameTree>>,
    set: Option<&WorkspaceSet>,
    workspaces: &mut Query<'_, '_, &mut Workspace>,
) {
    if let Some(set) = set {
        for entity in set.items.iter().copied() {
            if let Ok(mut workspace) = workspaces.get_mut(entity) {
                workspace.tree.remove_leaf(PanelId::Preview);
            }
        }
    }
    if let Some(tree) = tree.as_deref_mut() {
        tree.0.remove_leaf(PanelId::Preview);
    }
}

/// 把 preview 加进 / 移出 picker 列（[`Node::split_leaf_sized`] /
/// [`Node::remove_leaf`]；列宽和容器都不动）。
fn set_preview_in_tree(
    tree: &mut Option<ResMut<FrameTree>>,
    set: Option<&WorkspaceSet>,
    workspaces: &mut Query<'_, '_, &mut Workspace>,
    preview: bool,
) {
    edit_trees(tree, set, workspaces, |node| match preview {
        true => node.split_leaf_sized(
            PanelId::List,
            Split::Vertical,
            PanelId::Preview,
            NO_GAP,
            Size::Proportion(2.0),
            Size::Proportion(1.0),
        ),
        false => node.remove_leaf(PanelId::Preview),
    });
}

/// `Picker` 一加进 world 就按创建顺序入队：观察者在 spawn / insert 的那一刻
/// 同步执行，命令里的多个 spawn 也按排队顺序触发，所以这是**创建顺序**。
fn enqueue_picker(add: On<Add<Picker>>, mut queue: ResMut<PickerQueue>) {
    queue.push(add.entity);
}

/// 有 picker 就在条带最右边加一列；没有就关掉。顺便按过滤词和折叠状态算可见行。
///
/// 开：根 `Split::Horizontal` 末尾追加 `Child`（`Picker` 容器 + 列内 `List`，
/// 列宽 = 条带的 1/3，见 [`Node::append_column`]），spawn 容器 + 列表 panel。
/// 关：摘掉这一列、despawn 三块面板、清 `PanelRoots` / `PanelRows` / `PanelSpawned`。
///
/// 多个 picker 同时存在时，只有 [`PickerQueue`] 的队首显示在这一列里；队首关闭 /
/// 被外部 despawn 后，同一个系统在同一帧把下一个接进来（列和面板复用），队列
/// 空了才摘列。关闭一个 picker 就是让它从世界消失：上游 harness 收到 `Pick`
/// 后负责 despawn，这里不做额外的事。
#[allow(clippy::too_many_arguments)]
pub fn sync_picker(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut spawned: ResMut<PanelSpawned>,
    mut state: ResMut<PickerInput>,
    mut queue: ResMut<PickerQueue>,
    mut tree: Option<ResMut<FrameTree>>,
    set: Option<Res<WorkspaceSet>>,
    mut workspaces: Query<&mut Workspace>,
    pickers: Query<(Entity, &Picker)>,
) {
    // 队列兜底：picker 在插件存在之前就 spawn 时观察者收不到；每帧把查询里
    // 漏掉的补到队尾（`push` 去重），再清掉已经 despawn / 关掉的。
    for (entity, _) in &pickers {
        queue.push(entity);
    }
    queue.retain(|entity| pickers.get(entity).is_ok());

    // 一次只显示一个：正在显示的那个只要还在就继续显示（后来的排队等它关）；
    // 它关闭 / 被外部 despawn 之后，队首接上。
    let shown = state
        .picker
        .and_then(|entity| pickers.get(entity).ok());
    let wanted = shown.or_else(|| queue.front().and_then(|entity| pickers.get(entity).ok()));
    match (wanted, state.column) {
        (Some((picker_entity, picker)), None) => {
            // 没有可改的树（只装 PickerPlugin 的最小 App）也照旧 spawn 面板：
            // 没有区域时 `picker_rows` 自己会跳过，不影响逻辑。
            let has_tree = tree.is_some()
                || set
                    .as_deref()
                    .is_some_and(|set| set.focused.is_some());
            let added = edit_trees(&mut tree, set.as_deref(), &mut workspaces, |node| {
                node.append_column_at(PanelId::Transcript, picker_column(false))
            });
            if has_tree && !added {
                // 树里已经有 picker 列（不该发生）：不 spawn 第二套面板。
                return;
            }
            let column = spawn_panel(
                &mut commands,
                PanelId::Picker,
                FramePanel::tree(),
                Look::new(
                    Color::srgb(0.07, 0.085, 0.12),
                    Color::srgb(0.95, 0.82, 0.35),
                    12.0,
                ),
            );
            // 列表 tile 透明：底色和边框由容器面板给，行只负责文字。
            let list = spawn_panel(
                &mut commands,
                PanelId::List,
                FramePanel::tree().layer(1),
                Look::new(Color::NONE, Color::NONE, 0.0),
            );
            roots.0.insert(PanelId::Picker, column);
            roots.0.insert(PanelId::List, list);
            rows.0.entry(PanelId::List).or_default();
            state.picker = Some(picker_entity);
            state.column = Some(column);
            state.list_panel = Some(list);
            state.filter.clear();
            state.ticked.clear();
            state.touched = false;
            state.more = false;
            state.scroll = 0;
            state.selected = picker.selected;
            // 一开始选中项上面的分支都是打开的（rig 的 `open`）。
            state.open = branches(picker, state.selected).collect();
            // 初始选中项的身份先记下来，`refill` 按它在可见行里找回同一项
            // （折叠着的分支会让条目下标和行号不同）。
            state.picked = identity(picker, picker.selected);
            state.rows.clear();
            refill(&mut state, picker);
        }
        (Some((picker_entity, picker)), Some(_)) => {
            if state.picker != Some(picker_entity) {
                state.picker = Some(picker_entity);
                state.filter.clear();
                state.ticked.clear();
                state.touched = false;
                state.more = false;
                state.scroll = 0;
                state.selected = picker.selected;
                state.open = branches(picker, state.selected).collect();
                state.picked = identity(picker, picker.selected);
                state.rows.clear();
                refill(&mut state, picker);
            } else if rows_for(picker, &state.filter, &state.open) != state.rows {
                // 条目被插件重新填过时，行也跟着变；选中项按身份找回来。
                refill(&mut state, picker);
            }
            if picker.multiple && state.touched {
                let ticked = state.ticked.clone();
                commands.entity(picker_entity).try_insert(Ticked(ticked));
            }
        }
        (None, Some(_)) => {
            // 列里有 preview 时，它是"列表 / preview"的竖分裂；先摘 preview 叶子，
            // 否则 `remove_column(List)` 只摘掉列表那一半，preview 会留下来当空列，
            // 树就不能逐字节复原了。没有 preview leaf 时这一步是 no-op。
            remove_previews_everywhere(&mut tree, set.as_deref(), &mut workspaces);
            remove_columns_everywhere(&mut tree, set.as_deref(), &mut workspaces);
            for (id, panel) in [
                (PanelId::Picker, state.column),
                (PanelId::List, state.list_panel),
                (PanelId::Preview, state.preview_panel),
            ] {
                if let Some(panel) = panel {
                    commands.entity(panel).despawn();
                    roots.0.remove(&id);
                    rows.0.remove(&id);
                    spawned.forget(id);
                }
            }
            *state = PickerInput::default();
        }
        (None, None) => {}
    }
}

/// 插件给 picker 挂了 `PickPreview::Text` 时，把这一列竖着切成"列表 / preview"；
/// preview 没了就把那一半摘掉，列表重新占满整列。
///
/// 两种状态都只是改同一棵树（[`Node::split_leaf_sized`] / [`Node::remove_leaf`]），
/// 不做任何浮层/叠放——preview 就是这一列下面的另一块 tile。
#[allow(clippy::too_many_arguments)]
pub fn sync_preview_panel(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut spawned: ResMut<PanelSpawned>,
    mut state: ResMut<PickerInput>,
    mut tree: Option<ResMut<FrameTree>>,
    set: Option<Res<WorkspaceSet>>,
    mut workspaces: Query<&mut Workspace>,
    previews: Query<&PickPreview>,
) {
    let wanted = state
        .picker
        .filter(|picker| matches!(previews.get(*picker), Ok(PickPreview::Text(_))));
    match (wanted, state.preview_panel) {
        (Some(_), None) => {
            if state.column.is_none() {
                return;
            }
            let panel = spawn_panel(
                &mut commands,
                PanelId::Preview,
                FramePanel::tree().layer(1),
                Look::new(
                    Color::srgb(0.06, 0.07, 0.10),
                    Color::srgb(0.45, 0.55, 0.75),
                    10.0,
                ),
            );
            roots.0.insert(PanelId::Preview, panel);
            rows.0.entry(PanelId::Preview).or_default();
            state.preview_panel = Some(panel);
            set_preview_in_tree(&mut tree, set.as_deref(), &mut workspaces, true);
        }
        (None, Some(panel)) => {
            commands.entity(panel).despawn();
            roots.0.remove(&PanelId::Preview);
            rows.0.remove(&PanelId::Preview);
            spawned.forget(PanelId::Preview);
            state.preview_panel = None;
            set_preview_in_tree(&mut tree, set.as_deref(), &mut workspaces, false);
        }
        _ => {}
    }
}

/// 把选中条目报给插件（rig-tui 的 `mark_highlighted`）。
pub fn mark_highlighted(
    mut commands: Commands,
    state: Res<PickerInput>,
    highlighted: Query<Option<&Highlighted>>,
) {
    let Some(picker) = state.picker else {
        return;
    };
    let want = state.rows.get(state.selected).copied().flatten();
    let shown = highlighted.get(picker).ok().flatten().map(|h| h.0);
    match (want, shown) {
        (Some(index), Some(shown)) if shown == index => {}
        (None, None) => {}
        (Some(index), _) => {
            // 这一帧 picker 可能已经被 harness 收掉了（回车之后），不能直接 insert。
            commands.entity(picker).try_insert(Highlighted(index));
        }
        (None, Some(_)) => {
            commands.entity(picker).try_remove::<Highlighted>();
        }
    }
}

/// `code` 是哪个修饰键的位（[`Modifiers::is_modifier`] 为真时用）。
fn modifier_bit(code: KeyCode) -> Modifiers {
    match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Modifiers::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => Modifiers::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => Modifiers::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => Modifiers::SUPER,
        _ => Modifiers::NONE,
    }
}

/// 一次按键事件里的数字（Alt+1..9/Alt+0）：先看逻辑键（字符键是数字就是），
/// 逻辑键认不出来时退回物理键码。
fn digit_of(event: &KeyboardInput) -> Option<u8> {
    if let Key::Character(chars) = &event.logical_key {
        let mut characters = chars.chars();
        if let (Some(digit), None) = (characters.next(), characters.next())
            && digit.is_ascii_digit()
        {
            return Some(digit as u8 - b'0');
        }
        // 有些布局里 Alt+1 的逻辑键是别的字符（macOS 的 `¡`）：退回物理键码。
    }
    match event.key_code {
        KeyCode::Digit0 | KeyCode::Numpad0 => Some(0),
        KeyCode::Digit1 | KeyCode::Numpad1 => Some(1),
        KeyCode::Digit2 | KeyCode::Numpad2 => Some(2),
        KeyCode::Digit3 | KeyCode::Numpad3 => Some(3),
        KeyCode::Digit4 | KeyCode::Numpad4 => Some(4),
        KeyCode::Digit5 | KeyCode::Numpad5 => Some(5),
        KeyCode::Digit6 | KeyCode::Numpad6 => Some(6),
        KeyCode::Digit7 | KeyCode::Numpad7 => Some(7),
        KeyCode::Digit8 | KeyCode::Numpad8 => Some(8),
        KeyCode::Digit9 | KeyCode::Numpad9 => Some(9),
        _ => None,
    }
}

/// Alt+`number` 在当前 picker 上做什么（rig 的 `answer` 的数字版）：`0` 取消
/// （和 Esc 一样交 `Pick::person(picker, None)`），`n` 直接选并确认第 n 个
/// 可见项（和点它一样交 `Chosen::item`）。没有第 n 项时什么也不做——free_text
/// 的作答行不是条目，不能点，也不能用数字“确认”。
fn choice_key(number: u8, state: &PickerInput) -> Option<Option<Chosen>> {
    match number {
        0 => Some(None),
        number => {
            let index = state.rows.get(usize::from(number) - 1).copied().flatten()?;
            Some(Some(Chosen::item(index)))
        }
    }
}

/// picker 显示时的键位：打字过滤、↑↓ 选、PgUp/PgDn 翻页、←/→（树里折叠，
/// 平表里展开/收起说明）、Tab 打勾、回车、Esc，以及 Alt+1..9 直接选第 n 项、
/// Alt+0 取消。
///
/// 整个系统只在 `state.picker` 有值时跑：没有 picker 时 Alt+1..9/Alt+0 的
/// 事件一行都不读，仍由 `editor::keys` 交给 Ask 的内联绑定或插件绑定
/// （和 `keys::arm_inline_keys` 同一条边界）。
pub fn picker_keys(
    mut input: MessageReader<KeyboardInput>,
    mut state: ResMut<PickerInput>,
    mut pickers: Query<&mut Picker>,
    mut commands: Commands,
    held: Option<Res<ButtonInput<KeyCode>>>,
) {
    let Some(picker_entity) = state.picker else {
        return;
    };
    let Ok(mut picker) = pickers.get_mut(picker_entity) else {
        return;
    };
    let tree = tree(&picker);
    // 和 `editor::keys` 一样逐事件跟踪修饰键：同一帧里先按 Alt 再按数字
    // 也能算 Alt+数字，不依赖 `ButtonInput` 的帧尾状态。
    let mut modifiers = held.as_deref().map(Modifiers::from_held).unwrap_or_default();
    let mut dirty = false;
    for event in input.read() {
        if Modifiers::is_modifier(event.key_code) {
            match event.state {
                ButtonState::Pressed => modifiers.insert(modifier_bit(event.key_code)),
                ButtonState::Released => modifiers.remove(modifier_bit(event.key_code)),
            }
            continue;
        }
        if event.state != ButtonState::Pressed {
            continue;
        }
        // Alt+1..9 / Alt+0：picker 显示时由这里接管。Ctrl/Super 一起按着
        // 不算（那是别的绑定），按住不放的重复事件也只认一次。
        if modifiers.contains(Modifiers::ALT)
            && !modifiers.intersects(Modifiers::CONTROL | Modifiers::SUPER)
            && !event.repeat
            && let Some(number) = digit_of(event)
        {
            match choice_key(number, &state) {
                Some(Some(chosen)) => {
                    commands.trigger(Pick::person(picker_entity, Some(chosen)));
                }
                Some(None) => {
                    commands.trigger(Pick::person(picker_entity, None));
                }
                None => {}
            }
            continue;
        }
        let last = state.rows.len().saturating_sub(1);
        let typing = !modifiers.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER);
        match &event.logical_key {
            Key::ArrowUp => {
                state.selected = state.selected.saturating_sub(1);
                dirty = true;
            }
            Key::ArrowDown => {
                state.selected = (state.selected + 1).min(last);
                dirty = true;
            }
            Key::PageUp => {
                state.selected = state.selected.saturating_sub(PAGE);
                dirty = true;
            }
            Key::PageDown => {
                state.selected = (state.selected + PAGE).min(last);
                dirty = true;
            }
            Key::ArrowRight if tree => {
                unfold(&mut state, &picker, true);
                dirty = true;
            }
            Key::ArrowLeft if tree => {
                unfold(&mut state, &picker, false);
                dirty = true;
            }
            // 平表：→ 展开选中项的完整说明，再按一次收起；← 也收起。
            // （上游把它交给插件的 `ChoiceKeys`；本 crate 还没有那套自定义
            // 键，所以先给 → 一个自带的切换键。）
            Key::ArrowRight => {
                state.more = !state.more;
                dirty = true;
            }
            Key::ArrowLeft => {
                if state.more {
                    state.more = false;
                    dirty = true;
                }
            }
            Key::Tab if picker.multiple => {
                tick(&mut state, &picker);
                dirty = true;
            }
            Key::Backspace => {
                state.filter.pop();
                refilter(&mut state, &picker);
                dirty = true;
            }
            Key::Character(chars) if !chars.is_empty() && typing => {
                match &event.text {
                    Some(text) => state.filter.push_str(text),
                    None => state.filter.push_str(chars),
                }
                refilter(&mut state, &picker);
                dirty = true;
            }
            Key::Space if state.filter.len() < 200 && typing => {
                state.filter.push(' ');
                refilter(&mut state, &picker);
                dirty = true;
            }
            Key::Enter => {
                if let Some(chosen) = answer(&state, &picker) {
                    commands.trigger(Pick::person(picker_entity, Some(chosen)));
                }
            }
            Key::Escape => {
                commands.trigger(Pick::person(picker_entity, None));
            }
            _ => {}
        }
    }
    if dirty {
        // `Picker::selected` 是**条目下标**（不是过滤后的行号），只在选中条目时写。
        if let Some(Some(index)) = state.rows.get(state.selected).copied() {
            picker.selected = index;
        }
        // 下次过滤/重新填充按这个身份找回选中项。
        state.picked = identity_of(&state, &picker);
    }
}

/// 把当前 picker 画成浮层的行：标题、说明、输入行、列表、键位行。
pub fn picker_rows(
    mut panels: ResMut<PanelRows>,
    mut state: ResMut<PickerInput>,
    pickers: Query<&Picker>,
    theme: Res<Theme>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
) {
    let Some(picker_entity) = state.picker else {
        return;
    };
    let Ok(picker) = pickers.get(picker_entity) else {
        return;
    };
    let Some(area) = roots
        .entity(PanelId::List)
        .and_then(|root| areas.get(root))
    else {
        return;
    };

    let title_style = Style::default()
        .fg(RColor::LightYellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(RColor::Rgb(140, 148, 166));
    let fg = Style::default().fg(RColor::Rgb(214, 220, 232));
    let selected_style = Style::default()
        .fg(RColor::LightCyan)
        .add_modifier(Modifier::BOLD);

    let mut rows: Vec<KeyedRow> = Vec::new();
    let keyed = |key: u64, hash: u64, row: Row| KeyedRow { key, hash, row };

    rows.push(keyed(
        key_of(&["title"]),
        key_of(&[picker.title.as_str(), "title"]),
        Row::new(Line::from(vec![
            span("\u{258c} ", title_style),
            span(picker.title.clone(), title_style),
        ])),
    ));
    // 说明按列宽折行（rig 的 `wrap_all`，同样是 rig 的代码）。
    let body_lines: Vec<Line<'static>> = picker
        .body
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    let wrapped = rig_tui::wrap::wrap_all(
        &body_lines,
        usize::from(area.cols).saturating_sub(2).max(1),
    );
    for (index, mut line) in wrapped.into_iter().enumerate() {
        line.spans.insert(0, span("  ", dim));
        let hash = key_of(&[&line.to_string(), "body"]);
        rows.push(keyed(
            key_of(&["body", &index.to_string()]),
            hash,
            Row::new(line),
        ));
    }
    if !picker.body.is_empty() {
        rows.push(keyed(key_of(&["gap-body"]), 0, Row::new(Line::from(""))));
    }

    // 输入行：secret 的 picker 按字符显示成 `*`（字体子集里没有 • 这类符号）。
    if !state.filter.is_empty() || picker.free_text {
        let typed = match picker.secret {
            true => "*".repeat(state.filter.chars().count()),
            false => state.filter.clone(),
        };
        rows.push(keyed(
            key_of(&["typed"]),
            key_of(&[&typed, "typed"]),
            Row::new(Line::from(vec![
                span("  > ", dim),
                span(typed, fg),
                span("\u{258f}", selected_style),
            ])),
        ));
    }

    let used = rows.len();
    let rows_left = usize::from(area.rows).saturating_sub(used + 2);
    let visible = rows_left.max(1);
    let tree = tree(&picker);
    // `more`：选中项的完整说明（`more`，没有就用 `detail`）按列宽折行，紧跟在
    // 选中行下面；窗口把第一行也尽量露出来（`tail`），再大就只保住选中行。
    let more = match (state.more, state.rows.get(state.selected).copied().flatten()) {
        (true, Some(index)) => more_text(&picker, index)
            .map(|text| more_lines(text, usize::from(area.cols), dim))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let total = state.rows.len() + more.len();
    // 列表体的第一行（rig 的 `scroll`）：选中行一定在窗口里，首尾都夹住。
    let first = scroll_to_selected(&mut state, visible, total, usize::from(!more.is_empty()));
    let mut drawn = 0usize;
    for (row_index, item_index) in state.rows.iter().enumerate().skip(first) {
        if drawn >= visible {
            break;
        }
        let selected = row_index == state.selected;
        let marker = if selected { "> " } else { "  " };
        let marker_style = if selected { selected_style } else { dim };
        let mut spans = vec![span(marker, marker_style)];
        let (label, detail, key_hash) = match item_index {
            Some(index) => {
                let item = &picker.items[*index];
                // 树：折叠标记（字体子集没有 ▸/▾，用 + / -）。
                if tree {
                    let folded = !state.open.contains(index);
                    let mark = match is_branch(&picker, *index) {
                        true if folded => "+ ",
                        true => "- ",
                        false => "  ",
                    };
                    spans.push(span(mark, dim));
                }
                if picker.multiple {
                    let box_text = match tick_box(&picker, *index, &state.ticked) {
                        TickBox::On => "[x] ",
                        TickBox::Mixed => "[-] ",
                        TickBox::Off => "[ ] ",
                    };
                    spans.push(span(box_text, dim));
                }
                (
                    item.label.clone(),
                    item.detail.clone(),
                    key_of(&[
                        "item",
                        &index.to_string(),
                        item.detail.as_deref().unwrap_or(""),
                        &selected.to_string(),
                        &format!("{:?}", tick_box(&picker, *index, &state.ticked)),
                        &state.open.contains(index).to_string(),
                    ]),
                )
            }
            None => (
                match picker.secret {
                    true => "（输入的就是答案）".to_string(),
                    false => "（用输入的文字作答案）".to_string(),
                },
                None,
                key_of(&["typed-row", &selected.to_string()]),
            ),
        };
        let label_style = if selected { selected_style } else { fg };
        spans.push(span(label, label_style));
        if let Some(detail) = detail {
            spans.push(span(format!("  {detail}"), dim));
        }
        let mut row = Row::new(Line::from(spans));
        if let Some(index) = item_index {
            row = row.picker_choice(picker_entity, *index);
        }
        if selected {
            row = row.band(theme.band);
        }
        rows.push(keyed(key_of(&["row", &row_index.to_string()]), key_hash, row));
        drawn += 1;
        if row_index != state.selected {
            continue;
        }
        // 展开的说明：不点选、不套选中底色，只是选中行下面的几行文字。
        for (line_index, line) in more.iter().enumerate() {
            if drawn >= visible {
                break;
            }
            rows.push(keyed(
                key_of(&["more", &row_index.to_string(), &line_index.to_string()]),
                key_of(&[&line.to_string(), "more"]),
                Row::new(line.clone()),
            ));
            drawn += 1;
        }
    }
    if state.rows.is_empty() {
        rows.push(keyed(
            key_of(&["empty"]),
            key_of(&[&state.filter, "empty"]),
            Row::new(Line::from(span("  （没有匹配的条目）", dim))),
        ));
    }

    rows.push(keyed(key_of(&["gap-keys"]), 0, Row::new(Line::from(""))));
    // 键位行按列表列的宽度丢低优先级的（hints::fit_keys）。
    let width = usize::from(area.cols);
    let mut keys: Vec<(&'static str, u8)> = vec![("↑↓ 选择", 9), ("Enter 确认", 10), ("Esc 关闭", 8)];
    if !picker.items.is_empty() {
        keys.push(("打字过滤", 7));
        keys.push(("Alt+1..9 选", 5));
    }
    if picker.multiple {
        keys.push(("Tab 打勾", 6));
    }
    if tree {
        keys.push(("←/→ 折叠", 5));
    }
    // 平表里 → 展开/收起选中项的说明（树里 → 是折叠，没有这个提示）。
    let has_more = state
        .rows
        .get(state.selected)
        .copied()
        .flatten()
        .is_some_and(|index| more_text(&picker, index).is_some());
    if !tree && has_more {
        keys.push(match state.more {
            true => ("→ 收起", 6),
            false => ("→ 说明", 6),
        });
    }
    if picker.free_text {
        keys.push(("打字作答", 7));
    }
    let keys = crate::hints::fit_keys(&keys, width.saturating_sub(2));
    rows.push(keyed(
        key_of(&["keys"]),
        key_of(&[&keys, "keys"]),
        Row::new(Line::from(span(keys, dim))),
    ));

    panels.set(PanelId::List, rows);
}

/// preview 列的文本（rig 把 `PickPreview::Text` 用 `wrap_all` 按列宽折行）。
pub fn preview_rows(
    mut panels: ResMut<PanelRows>,
    state: Res<PickerInput>,
    previews: Query<&PickPreview>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    theme: Res<Theme>,
) {
    let Some(picker) = state.picker else {
        return;
    };
    let Some(area) = roots
        .entity(PanelId::Preview)
        .and_then(|root| areas.get(root))
    else {
        return;
    };
    let Ok(PickPreview::Text(text)) = previews.get(picker) else {
        return;
    };
    let text = text.clone();
    let dim = Style::default().fg(RColor::Rgb(150, 158, 176));
    let mut rows: Vec<KeyedRow> = vec![KeyedRow {
        key: key_of(&["preview-title"]),
        hash: 0,
        row: Row::new(Line::from(span("preview", theme.accent))),
    }];
    let lines: Vec<Line<'static>> = text.lines().map(|line| Line::from(line.to_string())).collect();
    for (index, line) in rig_tui::wrap::wrap_all(&lines, usize::from(area.cols).max(1))
        .into_iter()
        .enumerate()
    {
        rows.push(KeyedRow {
            key: key_of(&["preview", &index.to_string()]),
            hash: key_of(&[&line.to_string()]),
            row: Row::new(line.patch_style(dim)),
        });
    }
    panels.set(PanelId::Preview, rows);
}

fn answer_picker(
    click: On<bevy::picking::events::PointerClick>,
    choices: Query<&PickerChoice>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut entity = click.entity;
    let choice = loop {
        if let Ok(choice) = choices.get(entity) {
            break Some(choice.clone());
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => break None,
        }
    };
    let Some(choice) = choice else { return };
    commands.trigger(Pick::person(choice.picker, Some(Chosen::item(choice.index))));
}

pub struct PickerPlugin;

impl Plugin for PickerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PickerInput>()
            .init_resource::<PickerQueue>()
            .add_systems(
                Update,
                (
                    sync_picker,
                    sync_preview_panel,
                    picker_keys,
                    mark_highlighted,
                )
                    .chain()
                    .in_set(FeathersSystems::Prepare),
            )
            .add_systems(
                Update,
                (picker_rows, preview_rows).in_set(FeathersSystems::Draw),
            )
            .add_observer(answer_picker)
            .add_observer(enqueue_picker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::KeyCode;

    use crate::frame::{CELL, CellRect, PanelArea, PanelInset, UNITS_TO_WORLD};
    use crate::niri::TileRect;

    fn picker(items: &[(&str, Option<&str>)], free_text: bool) -> Picker {
        let mut picker = Picker::new("test");
        for (label, detail) in items {
            let mut item = PickItem::new(*label, "");
            if let Some(detail) = detail {
                item = item.detail(*detail);
            }
            picker.items.push(item);
        }
        picker.free_text = free_text;
        picker
    }

    fn tree_picker() -> Picker {
        let mut picker = Picker::new("tree");
        picker.items.push(PickItem::new("sessions", "")); // 0 分支
        let mut a = PickItem::new("today", ""); // 1
        a.parent = Some(0);
        picker.items.push(a);
        let mut b = PickItem::new("yesterday", ""); // 2
        b.parent = Some(0);
        picker.items.push(b);
        picker.items.push(PickItem::new("model", "")); // 3
        picker
    }

    #[test]
    fn filtering_keeps_items_whose_label_or_detail_holds_every_word() {
        let picker = picker(
            &[
                ("openai/gpt-5", None),
                ("deepseek/deepseek-chat", Some("cheap")),
                ("anthropic/claude", Some("deepseek too")),
            ],
            false,
        );
        // 只有 label 命中的排在前面；只在 detail 里命中的排在后面。
        assert_eq!(rows_for(&picker, "deep", &[]), vec![Some(1), Some(2)]);
        assert_eq!(rows_for(&picker, "cheap", &[]), vec![Some(1)]);
        assert_eq!(rows_for(&picker, "nothing", &[]), Vec::<Option<usize>>::new());
    }

    #[test]
    fn free_text_adds_the_typed_answer_row() {
        let picker = picker(&[("a", None)], true);
        assert_eq!(rows_for(&picker, "", &[]), vec![Some(0), None]);
        assert_eq!(rows_for(&picker, "zzz", &[]), vec![None]);
    }

    #[test]
    fn holds_needs_every_word() {
        assert!(holds("deep chat", &[Some("deepseek-chat")]));
        assert!(holds("", &[Some("anything")]));
        assert!(!holds("deep missing", &[Some("deepseek")]));
    }

    #[test]
    fn a_tree_hides_what_its_closed_branch_holds() {
        let picker = tree_picker();
        // 全关：只剩顶层两项。
        assert_eq!(rows_for(&picker, "", &[]), vec![Some(0), Some(3)]);
        // 打开 0：子项出现在它下面。
        assert_eq!(rows_for(&picker, "", &[0]), vec![Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn typing_in_a_tree_also_shows_the_branch_above_the_match() {
        let picker = tree_picker();
        // 没有展开任何分支，但命中项让它的分支现出来。
        assert_eq!(rows_for(&picker, "today", &[]), vec![Some(0), Some(1)]);
    }

    #[test]
    fn a_branch_tick_covers_what_sits_under_it() {
        let picker = tree_picker();
        let mut state = PickerInput {
            rows: rows_for(&picker, "", &[0]),
            ..Default::default()
        };
        // 选中分支 0 打勾：它和两个子项都进 ticked。
        state.selected = 0;
        tick(&mut state, &picker);
        assert_eq!(state.ticked, vec![0, 1, 2]);
        assert_eq!(tick_box(&picker, 0, &state.ticked), TickBox::On);
        // 再按一次：整枝取消。
        tick(&mut state, &picker);
        assert!(state.ticked.is_empty());
    }

    #[test]
    fn entering_a_multiple_picker_answers_with_the_ticked_items() {
        let mut picker = picker(&[("a", None), ("b", None)], false);
        picker.multiple = true;
        let mut state = PickerInput {
            rows: rows_for(&picker, "", &[]),
            ..Default::default()
        };
        state.selected = 1;
        tick(&mut state, &picker);
        assert_eq!(answer(&state, &picker), Some(Chosen::items([1])));
        // 没有动过勾时回车交的是选中的那一个。
        state.touched = false;
        assert_eq!(answer(&state, &picker), Some(Chosen::item(1)));
    }

    // -----------------------------------------------------------------------
    // 条带里的 picker 列（headless）
    // -----------------------------------------------------------------------

    /// 观察 `Picker` 回执，验证回车 / Esc 真的把选择报给了插件。
    #[derive(Resource, Default)]
    struct SeenPicks(Vec<(Entity, Option<Chosen>)>);

    fn tiled_app() -> App {
        let mut app = App::new();
        app.init_resource::<FrameTree>()
            .init_resource::<PanelRoots>()
            .init_resource::<PanelRows>()
            .init_resource::<PanelSpawned>()
            .init_resource::<PanelAreas>()
            .init_resource::<Theme>()
            .init_resource::<SeenPicks>()
            .add_message::<KeyboardInput>()
            // 生产里由 `FeathersViewPlugin` 排好；单独测 picker 时也要有同样的
            // Prepare → Draw 顺序，否则"关掉后同一帧换下一个"可能被 Draw 抢先。
            .configure_sets(
                Update,
                (
                    FeathersSystems::Prepare,
                    FeathersSystems::Layout,
                    FeathersSystems::Draw,
                    FeathersSystems::Fit,
                    FeathersSystems::Render,
                )
                    .chain(),
            )
            .add_plugins(PickerPlugin)
            .add_observer(|pick: On<Pick>, mut seen: ResMut<SeenPicks>| {
                seen.0.push((pick.entity, pick.chosen.clone()));
            });
        app
    }

    fn spawn_picker(app: &mut App, picker: Picker) -> Entity {
        let entity = app.world_mut().spawn(picker).id();
        app.update();
        entity
    }

    fn titled_picker(title: &str, label: &str) -> Picker {
        Picker::new(title).item(label, "")
    }

    /// 给列里的列表 tile 一块 `rect` 大小的区域，让 `picker_rows` 真的产出行。
    fn give_list_area(app: &mut App, rect: CellRect) -> Entity {
        let list = column_entities(app).1.expect("列表面板");
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(list, PanelArea::new(rect, PanelInset::DEFAULT));
        list
    }

    /// 默认区域（32 列 × 28 行）的便捷入口。
    fn give_list_an_area(app: &mut App) -> Entity {
        give_list_area(app, CellRect::new(64, 0, 32, 28))
    }

    /// 列表 tile 这一帧画出来的全部文字（标题、说明、条目、键位行）。
    fn list_text(app: &App) -> String {
        app.world()
            .resource::<PanelRows>()
            .get(PanelId::List)
            .iter()
            .map(|keyed| keyed.row.line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn queued(app: &App) -> Vec<Entity> {
        app.world()
            .resource::<PickerQueue>()
            .entities()
            .to_vec()
    }

    fn frame_tile(cols: u16, rows: u16) -> TileRect {
        TileRect {
            center: Vec2::ZERO,
            size: Vec2::new(f32::from(cols), f32::from(rows)) * CELL * UNITS_TO_WORLD,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn left(rect: &TileRect) -> f32 {
        rect.center.x - rect.size.x * 0.5
    }

    fn right(rect: &TileRect) -> f32 {
        rect.center.x + rect.size.x * 0.5
    }

    fn top(rect: &TileRect) -> f32 {
        rect.center.y + rect.size.y * 0.5
    }

    fn bottom(rect: &TileRect) -> f32 {
        rect.center.y - rect.size.y * 0.5
    }

    /// 叶子精确铺满帧、互不重叠。
    fn assert_tiles(rects: &[(PanelId, TileRect)], frame: TileRect) {
        let mut area = 0.0;
        for (index, (id, a)) in rects.iter().enumerate() {
            area += a.size.x * a.size.y;
            if a.size.x <= 0.0 || a.size.y <= 0.0 {
                // 默认树里补全 tile 关着时就是 0 高度：允许，只是不参与铺满。
                continue;
            }
            for (other, b) in rects.iter().skip(index + 1) {
                let overlap_x = left(a) < right(b) - 1e-9 && left(b) < right(a) - 1e-9;
                let overlap_y = bottom(a) < top(b) - 1e-9 && bottom(b) < top(a) - 1e-9;
                assert!(!(overlap_x && overlap_y), "{id:?} 和 {other:?} 重叠");
            }
            assert!(left(a) >= left(&frame) - 1e-9 && right(a) <= right(&frame) + 1e-9);
            assert!(bottom(a) >= bottom(&frame) - 1e-9 && top(a) <= top(&frame) + 1e-9);
        }
        assert!(
            close(area, frame.size.x * frame.size.y),
            "叶子面积之和 {area} != 帧面积"
        );
    }

    fn press(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn type_text(app: &mut App, text: &str, key_code: KeyCode) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Character(text.into()),
            state: ButtonState::Pressed,
            text: Some(text.into()),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    /// Alt+数字：先发修饰键、再发数字键（窗口事件不带修饰键字段，靠事件顺序
    /// 跟踪，和 `editor::keys` 的测试同一手法），最后松开 Alt。
    fn press_alt_digit(app: &mut App, digit: char) {
        let code = match digit {
            '0' => KeyCode::Digit0,
            '1' => KeyCode::Digit1,
            '2' => KeyCode::Digit2,
            '3' => KeyCode::Digit3,
            '4' => KeyCode::Digit4,
            '5' => KeyCode::Digit5,
            '6' => KeyCode::Digit6,
            '7' => KeyCode::Digit7,
            '8' => KeyCode::Digit8,
            '9' => KeyCode::Digit9,
            other => panic!("测试只用 Alt+0..9，实际 {other}"),
        };
        for (key_code, logical_key) in [
            (KeyCode::AltLeft, Key::Alt),
            (code, Key::Character(digit.to_string().into())),
        ] {
            app.world_mut().write_message(KeyboardInput {
                key_code,
                logical_key,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }
        app.update();
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::AltLeft,
            logical_key: Key::Alt,
            state: ButtonState::Released,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    /// 列表 tile 这一帧的行数（标题、说明、过滤、条目、键位行）。
    fn list_row_count(app: &App) -> usize {
        app.world().resource::<PanelRows>().get(PanelId::List).len()
    }

    fn column_entities(app: &App) -> (Option<Entity>, Option<Entity>, Option<Entity>) {
        let roots = app.world().resource::<PanelRoots>();
        (
            roots.entity(PanelId::Picker),
            roots.entity(PanelId::List),
            roots.entity(PanelId::Preview),
        )
    }

    /// 打开 picker：条带最右边多一列（叶子 +1），列宽 1/3、已有列等比缩到 2/3。
    #[test]
    fn opening_a_picker_appends_a_third_width_column_at_the_right() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        let frame = frame_tile(96, 28);
        let before_rects = before.rects(frame);
        assert_eq!(before_rects.len(), 7, "默认树 7 个叶子（转写/留白/确认/补全/输入/提示/状态）");

        let _picker = spawn_picker(
            &mut app,
            picker(&[("a", None), ("b", None)], false),
        );
        let tree = app.world().resource::<FrameTree>().0.clone();
        let after = tree.rects(frame);

        assert_eq!(after.len(), before_rects.len() + 1, "只多一列");
        // 列挂在主体那一行的末尾；整棵树后面还有提示行/状态行，所以按 PanelId 找。
        let column = after
            .iter()
            .find(|(id, _)| *id == PanelId::List)
            .expect("picker 列")
            .1;
        assert!(close(right(&column), right(&frame)), "列在主体行最右");
        assert!(close(column.size.x, frame.size.x / 3.0), "新列占 1/3");
        assert!(close(right(&column), right(&frame)), "新列贴住条带最右");
        assert_eq!(
            tree.containers(frame).last().map(|(id, _)| *id),
            Some(PanelId::Picker),
            "容器面板在列的位置"
        );
        assert_tiles(&after, frame);

        // 主体行里的列全部乘同一个因子 2/3（相对宽度不变），高度不变。
        // 注意 Hints / Status 是横跨整条的独立行（不在主体行里），它们不缩。
        for (id, old) in &before_rects {
            if matches!(*id, PanelId::Hints | PanelId::Status) {
                continue;
            }
            let new = after
                .iter()
                .find(|(found, _)| found == id)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{id:?} 还在树上"));
            assert!(
                close(new.size.x, old.size.x * 2.0 / 3.0),
                "{id:?} 按比例缩到 2/3（{old:?} → {new:?}）"
            );
            assert!(close(new.size.y, old.size.y));
            assert!(close(top(&new), top(old)));
        }

        // 面板实体登记齐全；行槽只建列表那一块。
        let (column, list, preview) = column_entities(&app);
        assert!(column.is_some() && list.is_some());
        assert!(preview.is_none(), "没有 PickPreview 就没有 preview tile");
        assert!(app
            .world()
            .resource::<PanelRows>()
            .0
            .contains_key(&PanelId::List));
    }

    /// 打开 picker 后 rects() 仍然精确铺满、不重叠（容器不参与叶子铺满）。
    #[test]
    fn the_strip_still_tiles_exactly_after_the_picker_column() {
        let mut app = tiled_app();
        let _picker = spawn_picker(
            &mut app,
            picker(&[("a", None), ("b", None), ("c", None)], false),
        );
        let tree = app.world().resource::<FrameTree>().0.clone();
        let frame = frame_tile(96, 28);
        assert_tiles(&tree.rects(frame), frame);
        // 容器面板和列表区域完全重合（容器垫在后面）。
        let column = tree.containers(frame).last().expect("容器").1;
        let list = tree
            .rects(frame)
            .into_iter()
            .find(|(id, _)| *id == PanelId::List)
            .expect("列表")
            .1;
        assert!(close(left(&column), left(&list)));
        assert!(close(right(&column), right(&list)));
        assert!(close(top(&column), top(&list)));
        assert!(close(bottom(&column), bottom(&list)));
    }

    /// 关闭：列摘掉、叶子数回到原值、三块面板实体 despawn、行槽没有残留。
    #[test]
    fn closing_the_picker_removes_the_column_and_its_panels() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        let (column, list, _) = column_entities(&app);
        let (column, list) = (column.expect("容器"), list.expect("列表"));
        assert_eq!(app.world().resource::<FrameTree>().0.leaves().len(), 8);

        app.world_mut().entity_mut(picker).despawn();
        app.update();

        assert_eq!(
            app.world().resource::<FrameTree>().0,
            before,
            "树恢复原样（叶子、权重、顺序都不差）"
        );
        assert_eq!(app.world().resource::<FrameTree>().0.leaves().len(), 7);
        assert!(app.world().get_entity(column).is_err(), "容器已 despawn");
        assert!(app.world().get_entity(list).is_err(), "列表已 despawn");
        let roots = app.world().resource::<PanelRoots>();
        assert!(roots.entity(PanelId::Picker).is_none());
        assert!(roots.entity(PanelId::List).is_none());
        assert!(roots.entity(PanelId::Preview).is_none());
        let rows = app.world().resource::<PanelRows>();
        for id in [PanelId::Picker, PanelId::List, PanelId::Preview] {
            assert!(!rows.0.contains_key(&id), "{id:?} 的行槽要清掉");
            assert!(
                !app.world().resource::<PanelSpawned>().0.contains_key(&id),
                "{id:?} 的已生成行要忘掉"
            );
        }
    }

    /// 开/关两次不重复加列：每次打开叶子数都一样，树每次关完都精确复原。
    #[test]
    fn opening_and_closing_twice_does_not_stack_columns() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        for round in 0..2 {
            let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
            let leaves = app.world().resource::<FrameTree>().0.leaves();
            assert_eq!(leaves.len(), 8, "第 {round} 次打开只加一列");
            assert_eq!(
                leaves.iter().filter(|id| **id == PanelId::List).count(),
                1,
                "列表叶子只有一个"
            );
            app.world_mut().entity_mut(picker).despawn();
            app.update();
            assert_eq!(
                app.world().resource::<FrameTree>().0,
                before,
                "第 {round} 次关完精确复原"
            );
        }
    }

    /// 有 `PickPreview::Text`：列内竖直分裂成列表 2 份 + preview 1 份。
    #[test]
    fn the_preview_splits_the_column_two_to_one() {
        let mut app = tiled_app();
        let picker = spawn_picker(&mut app, picker(&[("a", None), ("b", None)], false));
        let frame = frame_tile(96, 28);
        let no_preview = app.world().resource::<FrameTree>().0.clone();

        app.world_mut()
            .entity_mut(picker)
            .insert(PickPreview::Text("hello".to_owned()));
        app.update();

        let tree = app.world().resource::<FrameTree>().0.clone();
        let rects = tree.rects(frame);
        assert_eq!(rects.len(), 9, "默认 7 叶 + 容器里的列表 + preview");
        // picker 列挂在**主体那一行**的末尾；整棵树后面还有提示行/状态行，
        // 所以不能拿 `rects.last()` 当列，只能按 PanelId 找。
        let list = rects
            .iter()
            .find(|(id, _)| *id == PanelId::List)
            .expect("列表")
            .1;
        let preview = rects
            .iter()
            .find(|(id, _)| *id == PanelId::Preview)
            .expect("preview")
            .1;
        let container = tree.containers(frame).last().expect("容器").1;
        assert!(close(right(&list), right(&container)));
        assert!(close(right(&container), right(&frame)), "列贴条带最右");
        assert!(close(list.size.x, container.size.x));
        assert!(close(preview.size.x, container.size.x));
        assert!(close(list.size.y, container.size.y * 2.0 / 3.0), "列表 2/3");
        assert!(close(preview.size.y, container.size.y / 3.0), "preview 1/3");
        assert!(close(bottom(&list), top(&preview)), "上下相接、没有缝");
        assert!(close(top(&list), top(&container)));
        assert!(close(bottom(&preview), bottom(&container)));
        assert_tiles(&rects, frame);

        // 收起来的那棵树以后能恢复（preview 关掉时对比）。
        app.world_mut().entity_mut(picker).remove::<PickPreview>();
        app.update();
        assert_eq!(
            app.world().resource::<FrameTree>().0,
            no_preview,
            "preview 关掉后列内恢复成只有列表"
        );
    }

    /// preview 关闭：preview 面板 despawn，列表重新占满整列（容器整高）。
    #[test]
    fn closing_the_preview_gives_the_list_the_whole_column() {
        let mut app = tiled_app();
        let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        app.world_mut()
            .entity_mut(picker)
            .insert(PickPreview::Text("hello".to_owned()));
        app.update();
        let preview = column_entities(&app).2.expect("preview 面板");
        let frame = frame_tile(96, 28);

        app.world_mut().entity_mut(picker).remove::<PickPreview>();
        app.update();

        assert!(app.world().get_entity(preview).is_err(), "preview 已 despawn");
        let tree = app.world().resource::<FrameTree>().0.clone();
        let rects = tree.rects(frame);
        assert_eq!(rects.len(), 8, "默认 7 叶 + 列表（preview 已摘）");
        let container = tree.containers(frame).last().expect("容器").1;
        let list = rects
            .iter()
            .find(|(id, _)| *id == PanelId::List)
            .expect("列表")
            .1;
        assert!(close(right(&list), right(&frame)), "列表占满整列、贴条带最右");
        assert!(close(left(&list), left(&container)));
        assert!(close(right(&list), right(&container)));
        assert!(close(top(&list), top(&container)));
        assert!(close(bottom(&list), bottom(&container)));
        assert!(app.world().resource::<PanelRoots>().entity(PanelId::Preview).is_none());
        assert!(!app.world().resource::<PanelRows>().0.contains_key(&PanelId::Preview));
    }

    /// picker 的行写进列里的 `List` tile，不再有 Overlay 容器面板。
    #[test]
    fn the_picker_draws_its_rows_into_the_list_tile() {
        let mut app = tiled_app();
        let _picker = spawn_picker(&mut app, Picker::new("模型选择").item("gpt", "").item("claude", ""));
        let list = column_entities(&app).1.expect("列表面板");
        app.world_mut()
            .resource_mut::<PanelAreas>()
            .0
            .insert(list, PanelArea::new(CellRect::new(64, 0, 32, 28), PanelInset::DEFAULT));
        app.update();

        let rows = app.world().resource::<PanelRows>();
        let list_rows = rows.get(PanelId::List);
        assert!(!list_rows.is_empty(), "标题/列表行写到 List");
        assert!(
            list_rows.iter().any(|keyed| keyed
                .row
                .line
                .spans
                .iter()
                .any(|span| span.content.contains("模型选择"))),
            "标题行在 List tile 里"
        );
        assert!(
            rows.0.get(&PanelId::Picker).is_none(),
            "容器面板不产行（没有 Overlay 那一层）"
        );
    }

    /// ↑↓ 选、到边界夹住；选中条目写回 `Picker.selected`。
    #[test]
    fn arrow_keys_move_the_selection() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("a", None), ("b", None), ("c", None)], false),
        );
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        assert_eq!(app.world().resource::<PickerInput>().selected, 2);
        assert_eq!(app.world().get::<Picker>(picker).expect("picker").selected, 2);
        press(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
        assert_eq!(app.world().resource::<PickerInput>().selected, 1);
        for _ in 0..4 {
            press(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
        }
        assert_eq!(app.world().resource::<PickerInput>().selected, 0, "夹在顶部");
    }

    /// 打字过滤列表，退格恢复。
    #[test]
    fn typing_filters_the_list() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("alpha", None), ("beta", None), ("gamma", None)], false),
        );
        type_text(&mut app, "b", KeyCode::KeyB);
        app.update();
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.filter, "b");
            assert_eq!(state.rows, vec![Some(1)], "只剩 beta");
            assert_eq!(state.selected, 0);
        }
        assert_eq!(app.world().get::<Picker>(picker).expect("picker").selected, 1);
        press(&mut app, KeyCode::Backspace, Key::Backspace);
        assert_eq!(app.world().resource::<PickerInput>().filter, "");
        assert_eq!(app.world().resource::<PickerInput>().rows.len(), 3);
    }

    /// Tab 打勾（多选）：状态更新，下一帧把 `Ticked` 写回 picker；再按一次取消。
    #[test]
    fn tab_ticks_the_selected_item_in_a_multiple_picker() {
        let mut app = tiled_app();
        let mut picker = picker(&[("a", None), ("b", None)], false);
        picker.multiple = true;
        let picker = spawn_picker(&mut app, picker);
        press(&mut app, KeyCode::Tab, Key::Tab);
        assert_eq!(app.world().resource::<PickerInput>().ticked, vec![0]);
        assert!(app.world().resource::<PickerInput>().touched);
        app.update();
        assert_eq!(
            app.world().get::<Ticked>(picker).map(|ticked| ticked.0.clone()),
            Some(vec![0])
        );
        press(&mut app, KeyCode::Tab, Key::Tab);
        assert!(app.world().resource::<PickerInput>().ticked.is_empty());
        app.update();
        assert_eq!(
            app.world().get::<Ticked>(picker).map(|ticked| ticked.0.clone()),
            Some(Vec::new())
        );
    }

    /// 回车：交选中项（箭头先移动到第二项）。
    #[test]
    fn enter_answers_with_the_selected_item() {
        let mut app = tiled_app();
        let picker = spawn_picker(&mut app, picker(&[("a", None), ("b", None)], false));
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        press(&mut app, KeyCode::Enter, Key::Enter);
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, Some(Chosen::item(1))))
        );
    }

    /// Esc：关闭，不带选择。
    #[test]
    fn escape_closes_without_a_choice() {
        let mut app = tiled_app();
        let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        press(&mut app, KeyCode::Escape, Key::Escape);
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, None))
        );
    }

    /// Alt+1 直接选第 1 项并确认：和点击一样交 `Chosen::item(0)`，不往过滤
    /// 词里打字。
    #[test]
    fn alt_one_picks_the_first_item_like_a_click() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("a", None), ("b", None), ("c", None)], false),
        );
        press_alt_digit(&mut app, '1');
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, Some(Chosen::item(0)))),
            "Alt+1 = 点第一行：同一个 Pick/Chosen"
        );
        assert_eq!(
            app.world().resource::<PickerInput>().filter,
            "",
            "Alt+1 不往过滤词里打字符"
        );
    }

    /// Alt+3 选第 3 项并确认。
    #[test]
    fn alt_three_picks_the_third_item() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("a", None), ("b", None), ("c", None)], false),
        );
        press_alt_digit(&mut app, '3');
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, Some(Chosen::item(2))))
        );
    }

    /// Alt+0 取消：和 Esc 一样交 `Pick::person(picker, None)`。
    #[test]
    fn alt_zero_cancels_the_picker() {
        let mut app = tiled_app();
        let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        press_alt_digit(&mut app, '0');
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, None))
        );
    }

    /// 越界的 Alt+数字什么都不做（不 panic、不产生 Pick）。
    #[test]
    fn alt_past_the_last_item_answers_nothing() {
        let mut app = tiled_app();
        let _picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        press_alt_digit(&mut app, '3');
        assert!(app.world().resource::<SeenPicks>().0.is_empty());
    }

    /// 没有 picker 时 Alt+1..9/Alt+0 仍归输入框：插件的 Alt+1 绑定照常
    /// 收到 `KeyPressed`；picker 一出现才由 picker 接管（和 Ask 的
    /// `arm_inline_keys` 同一条边界）。
    #[test]
    fn alt_digits_stay_with_the_input_when_no_picker_shows() {
        #[derive(Resource, Default)]
        struct Pressed(bool);

        use crate::keys::AppKeyBindingsExt;

        let mut app = tiled_app();
        // 输入键分派（`editor::keys`）只在没有 picker 时跑，和 `EditorPlugin`
        // 的 `run_if(crate::picker::no_picker)` 一样。
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<crate::editor::Editor>()
            .init_resource::<crate::editor::SubmittedPrompts>()
            .init_resource::<crate::keys::KeyFocus>()
            .init_resource::<crate::keys::PendingInput>()
            .add_message::<crate::editor::PromptSubmitted>();
        crate::keys::add_built_in(&mut app);
        app.init_resource::<Pressed>();
        app.add_key_binding(
            crate::keys::KeyBinding::new(crate::keys::KeyPress::alt('1'), "Plugin")
                .in_scope(crate::keys::KeyScope::Global),
            |_pressed: On<crate::keys::KeyPressed>, mut seen: ResMut<Pressed>| seen.0 = true,
        );
        app.add_systems(Update, crate::editor::keys.run_if(crate::picker::no_picker));
        app.update();

        press_alt_digit(&mut app, '1');
        assert!(
            app.world().resource::<Pressed>().0,
            "没有 picker：Alt+1 不能被 picker 吞掉，插件绑定要收到 KeyPressed"
        );
        assert!(
            app.world().resource::<SeenPicks>().0.is_empty(),
            "没有 picker：Alt+1 不产生 Pick"
        );

        // picker 出现后，同样的键归 picker：直接确认第 1 项，插件绑定不再响。
        app.world_mut().resource_mut::<Pressed>().0 = false;
        let picker = spawn_picker(&mut app, picker(&[("a", None), ("b", None)], false));
        press_alt_digit(&mut app, '1');
        assert_eq!(
            app.world().resource::<SeenPicks>().0.last(),
            Some(&(picker, Some(Chosen::item(0)))),
            "picker 显示时 Alt+1 由 picker 接管"
        );
        assert!(
            !app.world().resource::<Pressed>().0,
            "picker 显示时 Alt+1 不再落到插件绑定"
        );
    }

    /// 带 `more` 的 picker：第二项有完整说明。
    fn picker_with_more() -> Picker {
        let mut picker = Picker::new("说明");
        picker
            .items
            .push(PickItem::new("short", "").detail("short detail"));
        let mut long = PickItem::new("long", "");
        long.detail = Some("one line".into());
        long.more = Some("the long description of the second item ".repeat(6));
        picker.items.push(long);
        picker
    }

    /// → 展开选中项的完整说明：列表体多出按列宽折行的说明行，再按一次收起。
    #[test]
    fn more_expands_and_collapses_the_selected_items_description() {
        let mut app = tiled_app();
        let _picker = spawn_picker(&mut app, picker_with_more());
        let list = give_list_an_area(&mut app);
        app.update();
        let before = list_row_count(&app);

        // 选中第二项（有 more 的那条）。
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        press(&mut app, KeyCode::ArrowRight, Key::ArrowRight);
        let expanded = list_row_count(&app);
        assert!(
            expanded > before,
            "展开后列表体要多出说明行：{before} → {expanded}"
        );

        let text = list_text(&app);
        assert!(
            text.contains("the long description of"),
            "展开的完整说明要画出来（按列宽折行）：{text}"
        );
        // 说明紧跟在选中行下面。
        let rows = app.world().resource::<PanelRows>().get(PanelId::List);
        let selected_at = rows
            .iter()
            .position(|keyed| keyed.row.line.to_string().contains("> long"))
            .expect("选中行");
        assert!(
            rows[selected_at + 1]
                .row
                .line
                .to_string()
                .contains("the long description of"),
            "说明行在选中行下面"
        );
        // 按列宽折行、不越界。
        let cols = usize::from(
            app.world()
                .resource::<PanelAreas>()
                .get(list)
                .expect("列表区域")
                .cols,
        );
        for keyed in rows {
            assert!(
                keyed.row.line.width() <= cols,
                "行宽 {} 越界（{cols} 列）：{}",
                keyed.row.line.width(),
                keyed.row.line
            );
        }
        // 说明行都有 4 格缩进（条目行的标记只有 2 格）。
        let wrapped = rows
            .iter()
            .filter(|keyed| keyed.row.line.to_string().starts_with("    "))
            .count();
        assert!(wrapped > 1, "长说明要折成多行（实际 {wrapped} 行）");

        // 再按一次 → 收起，行数恢复到展开前。
        press(&mut app, KeyCode::ArrowRight, Key::ArrowRight);
        assert_eq!(list_row_count(&app), before, "收起后行数恢复");
        assert!(!list_text(&app).contains("the long description of"));
    }

    /// 选项比可见行多时，列表体跟着选中项滚动；首尾都不越界。
    #[test]
    fn scrolling_follows_the_selection_and_stops_at_the_ends() {
        let mut app = tiled_app();
        let labels: Vec<String> = (0..20).map(|index| format!("item{index:02}")).collect();
        let items: Vec<(&str, Option<&str>)> =
            labels.iter().map(|label| (label.as_str(), None)).collect();
        let _picker = spawn_picker(&mut app, picker(&items, false));
        give_list_area(&mut app, CellRect::new(64, 0, 32, 12));
        app.update();
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.rows.len(), 20);
            assert_eq!(state.scroll, 0, "一打开从第一行开始");
        }

        // ↓ 到第 13 行：窗口滚到 6，选中行在最下面。
        for _ in 0..12 {
            press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        }
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 12);
            assert_eq!(state.scroll, 6, "窗口跟着选中项走");
        }
        let text = list_text(&app);
        assert!(text.contains("item12"), "选中项可见：{text}");
        assert!(!text.contains("item00"), "顶部已经滚出去了：{text}");

        // 到尾巴：选中最后一项，窗口停在最大位置，不越界。
        for _ in 0..10 {
            press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        }
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 19);
            assert_eq!(state.scroll, 13, "看得见 7 行：20 - 7");
        }
        let text = list_text(&app);
        assert!(text.contains("item19"), "最后一项可见：{text}");
        assert!(!text.contains("item00"));

        // 再按 ↓ 也不越界。
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 19);
            assert_eq!(state.scroll, 13);
        }

        // 回顶部：窗口回到 0，第一项可见。
        for _ in 0..30 {
            press(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
        }
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 0);
            assert_eq!(state.scroll, 0);
        }
        let text = list_text(&app);
        assert!(text.contains("item00"));
        assert!(!text.contains("item19"));

        // PgDn/PgUp 也是同一套滚动。
        press(&mut app, KeyCode::PageDown, Key::PageDown);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 10);
            assert_eq!(state.scroll, 4, "选中行在窗口里");
        }
        press(&mut app, KeyCode::PageUp, Key::PageUp);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.selected, 0);
            assert_eq!(state.scroll, 0);
        }
    }

    /// 过滤后选中项还在列表里就保持同一个 label；不在了回到第 0 行，不 panic。
    #[test]
    fn filtering_keeps_the_selected_label_when_it_still_matches() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("alpha", None), ("beta", None), ("gamma", None)], false),
        );
        give_list_an_area(&mut app);
        app.update();

        // 选中 beta。
        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.rows[state.selected], Some(1));
            assert_eq!(state.picked, Some((1, "beta".to_owned())));
        }

        // "g" 只剩 gamma：beta 不在列表里了 → 回到第 0 行。
        type_text(&mut app, "g", KeyCode::KeyG);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.filter, "g");
            assert_eq!(state.rows, vec![Some(2)]);
            assert_eq!(state.selected, 0, "beta 不在过滤结果里：回到第一行");
            assert_eq!(
                app.world().get::<Picker>(picker).expect("picker").selected,
                2,
                "选中的现在是 gamma"
            );
        }

        // 退格回到全部：选中的 gamma 还在 → 保持同一 label（它现在在第 2 行）。
        press(&mut app, KeyCode::Backspace, Key::Backspace);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.filter, "");
            assert_eq!(state.rows.len(), 3);
            assert_eq!(state.rows[state.selected], Some(2), "gamma 仍是选中项");
            assert_eq!(state.picked, Some((2, "gamma".to_owned())));
        }

        // 再过滤一次：同一 label 继续保持在选中行。
        type_text(&mut app, "g", KeyCode::KeyG);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.rows[state.selected], Some(2));
        }

        // 过滤到一个都不剩：选中行夹回 0，不 panic。
        type_text(&mut app, "x", KeyCode::KeyX);
        {
            let state = app.world().resource::<PickerInput>();
            assert_eq!(state.filter, "gx");
            assert!(state.rows.is_empty());
            assert_eq!(state.selected, 0);
            assert_eq!(state.picked, None);
        }
    }

    /// 插件重新填充条目后，选中项按身份（label，其次条目下标）找回同一项。
    #[test]
    fn refilling_the_items_keeps_the_selected_label() {
        let mut app = tiled_app();
        let picker = spawn_picker(
            &mut app,
            picker(&[("alpha", None), ("beta", None), ("gamma", None)], false),
        );
        give_list_an_area(&mut app);
        app.update();

        press(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        // 插件在列表前面插一项（下标全部后移一位）。
        app.world_mut()
            .get_mut::<Picker>(picker)
            .expect("picker")
            .items
            .insert(0, PickItem::new("new", ""));
        app.update();

        let state = app.world().resource::<PickerInput>();
        assert_eq!(state.rows.len(), 4);
        assert_eq!(state.rows[state.selected], Some(2), "beta 现在是条目 2");
        assert_eq!(
            app.world()
                .get::<Picker>(picker)
                .expect("picker")
                .items[2]
                .label,
            "beta"
        );
        assert_eq!(state.picked, Some((2, "beta".to_owned())));
    }

    /// 聚焦工作区的 `Workspace.tree` 也跟着加/摘列（不只改全局代理）。
    #[test]
    fn the_column_is_added_to_the_focused_workspace_tree_too() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        let workspace = app
            .world_mut()
            .spawn(Workspace {
                session: 1,
                tree: before.clone(),
                pose: Transform::IDENTITY,
                order: 0,
            })
            .id();
        let mut set = WorkspaceSet::default();
        set.items.push(workspace);
        set.focused = Some(workspace);
        app.world_mut().insert_resource(set);

        let picker = spawn_picker(&mut app, picker(&[("a", None)], false));
        let focused = app
            .world()
            .get::<Workspace>(workspace)
            .expect("workspace")
            .tree
            .clone();
        assert_eq!(focused.leaves().len(), before.leaves().len() + 1);
        assert!(focused.leaves().contains(&PanelId::List));

        app.world_mut().entity_mut(picker).despawn();
        app.update();
        assert_eq!(
            app.world().get::<Workspace>(workspace).expect("workspace").tree,
            before,
            "关掉后工作区的树也复原"
        );
    }

    // -----------------------------------------------------------------------
    // 多个 picker：按创建顺序排队，一次显示一个
    // -----------------------------------------------------------------------

    /// 三个 picker 同时存在：列里只有第一个；关掉第一个后第二个在同一帧接上，
    /// 最后关完时列被摘掉、树逐字节复原。
    #[test]
    fn pickers_show_one_at_a_time_in_creation_order() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();

        // 同一帧创建三个（中间不 update）：队列必须是创建顺序，而不是查询顺序。
        let first = app.world_mut().spawn(titled_picker("第一个", "a")).id();
        let second = app.world_mut().spawn(titled_picker("第二个", "b")).id();
        let third = app.world_mut().spawn(titled_picker("第三个", "c")).id();
        app.update();
        let list = give_list_an_area(&mut app);
        app.update();

        assert_eq!(queued(&app), vec![first, second, third], "按创建顺序排队");
        assert_eq!(
            app.world().resource::<PickerInput>().picker,
            Some(first),
            "一次只显示第一个"
        );
        let text = list_text(&app);
        assert!(text.contains("第一个"), "第一个的行在列里：{text}");
        assert!(
            !text.contains("第二个") && !text.contains("第三个"),
            "后两个排队、不可见：{text}"
        );
        assert_eq!(
            app.world()
                .resource::<FrameTree>()
                .0
                .leaves()
                .iter()
                .filter(|panel| **panel == PanelId::List)
                .count(),
            1,
            "三个 picker 只有一列"
        );

        // 关掉第一个（harness 收到 Pick 后就是 despawn）：第二个在同一帧接上，
        // 复用同一块列面板，不留下空列。
        app.world_mut().entity_mut(first).despawn();
        app.update();
        assert_eq!(queued(&app), vec![second, third], "关掉的第一个出队");
        assert_eq!(app.world().resource::<PickerInput>().picker, Some(second));
        assert_eq!(
            column_entities(&app).1,
            Some(list),
            "列表面板复用，不是重建/空列"
        );
        let text = list_text(&app);
        assert!(text.contains("第二个"), "第二个的行接上了：{text}");
        assert!(!text.contains("第一个") && !text.contains("第三个"));

        // 第二个被外部 despawn（不是它自己交的答案）：第三个一样接上。
        app.world_mut().entity_mut(second).despawn();
        app.update();
        assert_eq!(queued(&app), vec![third]);
        assert_eq!(app.world().resource::<PickerInput>().picker, Some(third));
        assert!(list_text(&app).contains("第三个"));

        // 关掉最后一个：队列空、列摘掉、树逐字节复原、行槽/根表清干净。
        let (column, last_list, preview) = column_entities(&app);
        let (column, last_list) = (column.expect("容器"), last_list.expect("列表"));
        assert!(preview.is_none());
        app.world_mut().entity_mut(third).despawn();
        app.update();

        assert!(app.world().resource::<PickerQueue>().is_empty());
        assert!(!app.world().resource::<PickerInput>().is_open());
        assert_eq!(
            app.world().resource::<FrameTree>().0,
            before,
            "队列空后树与加列前逐字节相同"
        );
        assert_eq!(app.world().resource::<FrameTree>().0.leaves().len(), 7);
        assert!(app.world().get_entity(column).is_err(), "容器已 despawn");
        assert!(app.world().get_entity(last_list).is_err(), "列表已 despawn");
        assert!(app.world().resource::<PanelRoots>().entity(PanelId::List).is_none());
        assert!(!app.world().resource::<PanelRows>().0.contains_key(&PanelId::List));
        assert!(!app.world().resource::<PanelSpawned>().0.contains_key(&PanelId::List));
    }

    /// 同一个 picker 被重复入队（观察者重放 / 每帧兜底扫描）只排一次。
    #[test]
    fn the_same_picker_is_never_queued_twice() {
        let mut app = tiled_app();
        let first = spawn_picker(&mut app, titled_picker("一", "a"));
        let second = spawn_picker(&mut app, titled_picker("二", "b"));

        // 模拟重复的 spawn/add 事件：再入队两次必须是 no-op。
        {
            let mut queue = app.world_mut().resource_mut::<PickerQueue>();
            queue.push(first);
            queue.push(second);
            queue.push(first);
        }
        app.update();

        assert_eq!(queued(&app), vec![first, second]);
        assert_eq!(
            app.world()
                .resource::<FrameTree>()
                .0
                .leaves()
                .iter()
                .filter(|panel| **panel == PanelId::List)
                .count(),
            1,
            "重复入队不会追加第二列"
        );
        assert_eq!(
            app.world().resource::<PickerInput>().picker,
            Some(first),
            "重复入队也不改当前显示的那个"
        );
    }

    /// Esc 交给 harness（`Pick` → despawn）之后，队列下一个在同一/下一帧接上。
    #[test]
    fn escape_hands_the_column_to_the_next_picker() {
        let mut app = tiled_app();
        // 模拟 harness 的 `PickPlugin`：收到 `Pick` 就把那个 picker despawn。
        app.add_observer(|pick: On<Pick>, mut commands: Commands| {
            commands.entity(pick.entity).try_despawn();
        });
        let first = spawn_picker(&mut app, titled_picker("一", "a"));
        let second = spawn_picker(&mut app, titled_picker("二", "b"));
        let list = give_list_an_area(&mut app);
        app.update();

        press(&mut app, KeyCode::Escape, Key::Escape);
        // Pick 的 despawn 在帧末生效：再推一帧，队列在 sync_picker 里接上。
        app.update();

        assert!(
            app.world().get_entity(first).is_err(),
            "Esc 后第一个被 harness 关掉"
        );
        assert_eq!(queued(&app), vec![second]);
        assert_eq!(app.world().resource::<PickerInput>().picker, Some(second));
        assert_eq!(
            column_entities(&app).1,
            Some(list),
            "交接复用列，没有先摘再建"
        );
        assert!(list_text(&app).contains("二"), "列里现在是第二个");
    }

    /// 列里有 preview 时关闭：preview 叶子也要摘掉，整列才干净（否则会留下
    /// 一块只有 preview 的空列）；树逐字节复原。
    #[test]
    fn closing_a_picker_whose_preview_is_open_removes_the_whole_column() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        let picker = spawn_picker(&mut app, titled_picker("带 preview", "a"));
        app.world_mut()
            .entity_mut(picker)
            .insert(PickPreview::Text("hello".to_owned()));
        app.update();

        let tree = app.world().resource::<FrameTree>().0.clone();
        assert!(
            tree.leaves().contains(&PanelId::Preview),
            "preview 是列里的第二块 tile"
        );
        let (column, list, preview) = column_entities(&app);
        let (column, list, preview) = (
            column.expect("容器"),
            list.expect("列表"),
            preview.expect("preview"),
        );

        app.world_mut().entity_mut(picker).despawn();
        app.update();

        assert_eq!(
            app.world().resource::<FrameTree>().0,
            before,
            "整列摘掉，preview 不剩空列"
        );
        let leaves = app.world().resource::<FrameTree>().0.leaves();
        assert!(!leaves.contains(&PanelId::List));
        assert!(!leaves.contains(&PanelId::Preview));
        assert!(app.world().get_entity(column).is_err());
        assert!(app.world().get_entity(list).is_err());
        assert!(app.world().get_entity(preview).is_err());
        for id in [PanelId::Picker, PanelId::List, PanelId::Preview] {
            assert!(app.world().resource::<PanelRoots>().entity(id).is_none(), "{id:?} 根已清");
            assert!(!app.world().resource::<PanelRows>().0.contains_key(&id), "{id:?} 行槽已清");
            assert!(
                !app.world().resource::<PanelSpawned>().0.contains_key(&id),
                "{id:?} 已生成行已忘"
            );
        }
    }

    /// `Ask` 是条带里的确认 tile，不是 picker 列：它绝不进 picker 队列
    /// （这里和上游 `queue_choices` 把 Picker/Ask 放同一个队列是有意不同）。
    #[test]
    fn an_ask_never_enters_the_picker_queue() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        app.world_mut().spawn(
            rig_harness::prelude::Ask::new("确认支付？")
                .choice("yes", "确认")
                .choice("no", "取消"),
        );
        app.update();

        assert!(
            app.world().resource::<PickerQueue>().is_empty(),
            "Ask 不排进 picker 队列"
        );
        assert!(!app.world().resource::<PickerInput>().is_open());
        assert_eq!(
            app.world().resource::<FrameTree>().0,
            before,
            "Ask 不占 picker 列（它是确认 tile）"
        );
        assert!(app.world().resource::<PanelRoots>().entity(PanelId::List).is_none());
    }

    /// 切过工作区再关：picker 列开在旧树上，旧树的 preview 叶子和列都要清掉。
    #[test]
    fn closing_after_a_focus_switch_clears_the_old_workspaces_column() {
        let mut app = tiled_app();
        let before = app.world().resource::<FrameTree>().0.clone();
        let first = app
            .world_mut()
            .spawn(Workspace {
                session: 1,
                tree: before.clone(),
                pose: Transform::IDENTITY,
                order: 0,
            })
            .id();
        let second = app
            .world_mut()
            .spawn(Workspace {
                session: 2,
                tree: before.clone(),
                pose: Transform::IDENTITY,
                order: 1,
            })
            .id();
        let mut set = WorkspaceSet::default();
        set.items.extend([first, second]);
        set.focused = Some(first);
        app.world_mut().insert_resource(set);

        let picker = spawn_picker(&mut app, titled_picker("带 preview", "a"));
        app.world_mut()
            .entity_mut(picker)
            .insert(PickPreview::Text("hello".to_owned()));
        app.update();
        assert!(
            app.world()
                .get::<Workspace>(first)
                .expect("workspace")
                .tree
                .leaves()
                .contains(&PanelId::Preview)
        );

        // 焦点切到另一棵树上再关：旧树上的列（和 preview 叶子）也得摘干净。
        app.world_mut().resource_mut::<WorkspaceSet>().focused = Some(second);
        app.world_mut().entity_mut(picker).despawn();
        app.update();

        assert_eq!(
            app.world().get::<Workspace>(first).expect("workspace").tree,
            before,
            "旧工作区的树逐字节复原"
        );
        assert_eq!(
            app.world().get::<Workspace>(second).expect("workspace").tree,
            before,
            "没开过列的工作区不受影响"
        );
        assert_eq!(app.world().resource::<FrameTree>().0, before, "全局树也复原");
    }
}
