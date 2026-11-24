//! 列表选择器（picker）：rig 的插件用 `Picker` 让用户从列表里挑（会话、模型、命令…）。
//!
//! rig-tui 把 picker 画成**盖住整屏的浮层**：标题、说明、输入行、列表、底部键位行，
//! 选中项高亮，回车 `Pick::person(picker, Some(Chosen::item(i)))`，Esc 关闭；
//! 打字过滤列表（每个词都要出现，标签命中的排在前面），`free_text` 的 picker
//! 还能把打的字本身当答案；`multiple` 的 picker 用 Tab 打勾、回车交 `Chosen::items`；
//! 有 `PickItem::parent` 时是一棵树，←/→ 折叠；插件给 picker 挂 `PickPreview`
//! 时右边多一列 preview（帧布局的 `Placement::Within`）。
//!
//! 这里用帧布局的 `Over` 画主浮层、`Within { part: 1, of: 2 }` 画 preview 列——
//! 终端里浮层只能占满屏，3D 里它是浮在界面上方的一块对话框。

use bevy::prelude::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use rig_harness::pick::{PickItem, PickPreview, TickBox, Ticked};
use rig_harness::prelude::{Chosen, Highlighted, Pick, Picker};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::frame::{Extent, FramePanel, Look, PanelAreas, PanelRoots, Placement, spawn_panel};
use crate::panels::{KeyedRow, PanelId, PanelRows, PanelSpawned};
use crate::theme::Theme;
use crate::transcript::Row;
use crate::FeathersSystems;

/// rig-tui 的 `PAGE`：PgUp/PgDn 一次跳几个。
const PAGE: usize = 10;

/// 浮层在帧里的布局顺序（比默认屏幕的面板都晚）。
const PICKER_ORDER: u32 = 9;

/// 列表列面板的标记（要改它的 `Within` 宽度）。
#[derive(Component)]
pub struct ListPanel;

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
    /// 浮层的容器面板。
    pub panel: Option<Entity>,
    /// 列表列（容器里的 `Within`）。
    pub list_panel: Option<Entity>,
    /// preview 列（`PickPreview` 在时才有）。
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

/// 有 picker 就开浮层（容器 + 列表列），没有就关掉；顺便按过滤词和折叠状态算出可见行。
///
/// 结构照 rig-tui 的 picker 布局：**容器**占浮层那块地方，**列表列**是它的
/// `Within { part: 0, of }`（有 preview 时 `of = 2`，于是列表只占左半），
/// preview 是 `Within { part: 1, of: 2 }`。
pub fn sync_picker(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut spawned: ResMut<PanelSpawned>,
    mut state: ResMut<PickerInput>,
    pickers: Query<(Entity, &Picker)>,
) {
    let open = pickers.iter().next();
    match (open, state.panel) {
        (Some((picker_entity, picker)), None) => {
            let container = spawn_panel(
                &mut commands,
                PanelId::Overlay,
                FramePanel::new(Placement::Over {
                    width: Extent::Percent(84),
                    height: Extent::Percent(74),
                })
                .order(PICKER_ORDER),
                Look::new(
                    Color::srgb(0.07, 0.085, 0.12),
                    Color::srgb(0.95, 0.82, 0.35),
                    12.0,
                ),
            );
            // 列表列：默认占容器的全部（`of = 1`），有 preview 时切成一半。
            let list = spawn_panel(
                &mut commands,
                PanelId::List,
                FramePanel::new(Placement::Within {
                    panel: container,
                    part: 0,
                    of: 1,
                })
                .order(PICKER_ORDER + 1),
                Look::new(Color::NONE, Color::NONE, 0.0),
            );
            commands.entity(list).insert(ListPanel);
            roots.0.insert(PanelId::Overlay, container);
            roots.0.insert(PanelId::List, list);
            state.picker = Some(picker_entity);
            state.panel = Some(container);
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
            for (id, panel) in [
                (PanelId::Overlay, state.panel),
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

/// 插件给 picker 挂了 `PickPreview::Text` 时，右边开一列 preview（`Placement::Within`），
/// 同时把列表列从"整块容器"改成"左半"。
pub fn sync_preview_panel(
    mut commands: Commands,
    mut roots: ResMut<PanelRoots>,
    mut rows: ResMut<PanelRows>,
    mut spawned: ResMut<PanelSpawned>,
    mut state: ResMut<PickerInput>,
    mut lists: Query<&mut FramePanel, With<ListPanel>>,
    previews: Query<&PickPreview>,
) {
    let wanted = state
        .picker
        .filter(|picker| matches!(previews.get(*picker), Ok(PickPreview::Text(_))));
    let width = match wanted {
        Some(_) => 2,
        None => 1,
    };
    // 列表列的宽度跟着 preview 有无走。
    if let Some(list) = state.list_panel
        && let Ok(mut panel) = lists.get_mut(list)
        && let Placement::Within { part, of, .. } = &mut panel.placement
        && *of != width
    {
        *of = width;
        let _ = part;
    }
    match (wanted, state.preview_panel) {
        (Some(_), None) => {
            let Some(overlay) = roots.entity(PanelId::Overlay) else {
                return;
            };
            let panel = spawn_panel(
                &mut commands,
                PanelId::Preview,
                FramePanel::new(Placement::Within {
                    panel: overlay,
                    part: 1,
                    of: 2,
                })
                .order(PICKER_ORDER + 2),
                Look::new(
                    Color::srgb(0.06, 0.07, 0.10),
                    Color::srgb(0.45, 0.55, 0.75),
                    10.0,
                ),
            );
            roots.0.insert(PanelId::Preview, panel);
            state.preview_panel = Some(panel);
        }
        (None, Some(panel)) => {
            commands.entity(panel).despawn();
            roots.0.remove(&PanelId::Preview);
            rows.0.remove(&PanelId::Preview);
            spawned.forget(PanelId::Preview);
            state.preview_panel = None;
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

    panels.set(PanelId::Overlay, rows);
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
            .add_observer(answer_picker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
