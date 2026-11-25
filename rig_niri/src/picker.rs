//! 列表选择器（picker）：rig 的插件用 `Picker` 让用户从列表里挑（会话、模型、命令…）。
//!
//! rig-tui 把 picker 画成**盖住整屏的浮层**：标题、说明、输入行、列表、底部键位行，
//! 选中项高亮，回车 `Pick::person(picker, Some(Chosen::item(i)))`，Esc 关闭；
//! 打字过滤列表（每个词都要出现，标签命中的排在前面），`free_text` 的 picker
//! 还能把打的字本身当答案；`multiple` 的 picker 用 Tab 打勾、回车交 `Chosen::items`；
//! 有 `PickItem::parent` 时是一棵树，←/→ 折叠。
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
use bevy::input::keyboard::{Key, KeyboardInput};
use rig_harness::pick::{PickItem, PickPreview, TickBox, Ticked};
use rig_harness::prelude::{Chosen, Highlighted, Pick, Picker};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{FramePanel, FrameTree, Look, PanelAreas, PanelRoots, spawn_panel};
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

/// 重算可见行，并把选中行夹回范围里。
fn refresh(state: &mut PickerInput, picker: &Picker) {
    state.rows = rows_for(picker, &state.filter, &state.open);
    state.selected = state.selected.min(state.rows.len().saturating_sub(1));
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
            refresh(state, picker);
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
            refresh(state, picker);
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
            state.selected = picker.selected;
            // 一开始选中项上面的分支都是打开的（rig 的 `open`）。
            state.open = branches(picker, state.selected).collect();
            refresh(&mut state, picker);
        }
        (Some((picker_entity, picker)), Some(_)) => {
            if state.picker != Some(picker_entity) {
                state.picker = Some(picker_entity);
                state.filter.clear();
                state.ticked.clear();
                state.touched = false;
                state.selected = picker.selected;
                state.open = branches(picker, state.selected).collect();
            }
            // 条目被插件重新填过时，行也跟着变。
            let rows_now = rows_for(picker, &state.filter, &state.open);
            if rows_now != state.rows {
                state.rows = rows_now;
                state.selected = state.selected.min(state.rows.len().saturating_sub(1));
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

/// 浮层的键位：打字过滤、↑↓ 选、PgUp/PgDn 翻页、←/→ 折叠、Tab 打勾、回车、Esc。
pub fn picker_keys(
    mut input: MessageReader<KeyboardInput>,
    mut state: ResMut<PickerInput>,
    mut pickers: Query<&mut Picker>,
    mut commands: Commands,
) {
    let Some(picker_entity) = state.picker else {
        return;
    };
    let Ok(mut picker) = pickers.get_mut(picker_entity) else {
        return;
    };
    let tree = tree(&picker);
    let mut dirty = false;
    for event in input.read() {
        if event.state != bevy::input::ButtonState::Pressed {
            continue;
        }
        let last = state.rows.len().saturating_sub(1);
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
            Key::Tab if picker.multiple => {
                tick(&mut state, &picker);
                dirty = true;
            }
            Key::Backspace => {
                state.filter.pop();
                state.selected = 0;
                refresh(&mut state, &picker);
                dirty = true;
            }
            Key::Character(chars) if !chars.is_empty() => {
                match &event.text {
                    Some(text) => state.filter.push_str(text),
                    None => state.filter.push_str(chars),
                }
                state.selected = 0;
                refresh(&mut state, &picker);
                dirty = true;
            }
            Key::Space if state.filter.len() < 200 => {
                state.filter.push(' ');
                state.selected = 0;
                refresh(&mut state, &picker);
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
    }
}

/// 把当前 picker 画成浮层的行：标题、说明、输入行、列表、键位行。
pub fn picker_rows(
    mut panels: ResMut<PanelRows>,
    state: Res<PickerInput>,
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
    // 选中行滚到可见范围里（rig-tui 的 `rows` 也做同样的事）。
    let first = state
        .selected
        .saturating_sub(visible.saturating_sub(1))
        .min(state.rows.len().saturating_sub(visible));
    let tree = tree(&picker);
    for (row_index, item_index) in state.rows.iter().enumerate().skip(first).take(visible) {
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
    }
    if picker.multiple {
        keys.push(("Tab 打勾", 6));
    }
    if tree {
        keys.push(("←/→ 折叠", 5));
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

    /// 给列里的列表 tile 一块区域，让 `picker_rows` 真的产出行。
    fn give_list_an_area(app: &mut App) -> Entity {
        let list = column_entities(app).1.expect("列表面板");
        app.world_mut().resource_mut::<PanelAreas>().0.insert(
            list,
            PanelArea::new(CellRect::new(64, 0, 32, 28), PanelInset::DEFAULT),
        );
        list
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
