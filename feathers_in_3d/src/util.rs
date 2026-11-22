//! Shared UI-space helpers.

use bevy::prelude::*;
use bevy_ui::UiGlobalTransform;

/// Sums node-centre offsets from `panel` down to `leaf`, in physical pixels.
///
/// `UiGlobalTransform` translations are parent-relative, so the last hop (the row
/// itself) matters: missing it collapses every row onto the panel centre.
pub fn offset_from_panel(
    leaf: Entity,
    panel: Entity,
    leaf_ugt: &UiGlobalTransform,
    nodes: &Query<&UiGlobalTransform>,
    parents: &Query<&ChildOf>,
) -> Option<Vec2> {
    let mut offset = Vec2::ZERO;
    let mut current = leaf;
    let mut current_ugt = leaf_ugt;
    for _ in 0..64 {
        let parent = parents.get(current).ok()?.parent();
        let parent_ugt = nodes.get(parent).ok()?;
        offset += current_ugt.affine().translation - parent_ugt.affine().translation;
        if parent == panel {
            return Some(offset);
        }
        current = parent;
        current_ugt = parent_ugt;
    }
    None
}
