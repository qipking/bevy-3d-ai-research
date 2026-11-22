//! 键位提示：照抄 rig-tui `hints.rs` 的两组键位与优先级。
//!
//! rig-tui 的 `keys()`/`fit_keys()` 是 `pub(crate)`，行为是公开的：
//! 条目用 ` · ` 连接；行太窄时先丢 `keep` 最低的，末尾加 `…`。


use bevy::prelude::*;

/// 一条键位提示：文字 + 重要性（越高越不该丢）。
pub type Key = (&'static str, u8);

/// rig-tui 的两组键位提示（`hints::keys`）。
pub fn keys(turn_running: bool) -> &'static [Key] {
    match turn_running {
        true => &[
            ("Enter steers", 9),
            ("Tab sends after the turn", 7),
            ("Esc stops", 10),
            ("Shift+Enter or Ctrl+J new line", 5),
            ("Ctrl+V image", 1),
            ("/ commands", 4),
            ("@ files", 2),
            ("Ctrl+D quits", 3),
        ],
        false => &[
            ("Enter sends", 9),
            ("Shift+Enter or Ctrl+J new line", 7),
            ("Ctrl+V image", 4),
            ("/ commands", 8),
            ("@ files", 5),
            ("Ctrl+D quits", 6),
        ],
    }
}

/// 在 `width` 列里放下尽可能多的提示，丢掉的用 `…` 表示（rig-tui `fit_keys`）。
pub fn fit_keys(keys: &[Key], width: usize) -> String {
    let mut kept = keys.to_vec();
    loop {
        let mut items: Vec<&str> = kept.iter().map(|(item, _)| *item).collect();
        if kept.len() < keys.len() {
            items.push("…");
        }
        if items.is_empty() {
            return String::new();
        }
        let joined = items.join(" · ");
        if joined.chars().count() <= width {
            return joined;
        }
        // 丢掉重要性最低的一个
        let Some((index, _)) = kept
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, weight))| *weight)
        else {
            return joined;
        };
        kept.remove(index);
    }
}

/// 注册键位提示（与 rig-tui `KeyHintsPlugin` 同形：finish 里再初始化资源）。
pub struct HintsPlugin;

impl Plugin for HintsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeyHints>();
    }

    fn finish(&self, _app: &mut App) {}
}

/// 键位提示是否启用（rig-tui 用 `KeyHints` 资源表示，这里同名）。
#[derive(Resource, Default)]
pub struct KeyHints;

/// 当前该显示的键位提示（`turn_running` 由业务传入；模板里恒为 false）。
pub fn current_hints(width_cells: usize, turn_running: bool) -> String {
    fit_keys(keys(turn_running), width_cells)
}
