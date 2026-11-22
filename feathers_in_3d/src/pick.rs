//! Ray picking for world-space panels: controller/mouse ray -> panel plane -> UI node.
//!
//! This is the same three steps luma uses (`luma_interaction/.../hit_math.rs` +
//! `clip_sync::intersect_ray_with_node_plane`): intersect the node's plane, take the hit
//! into the panel's local space, then test the node's rectangle. Instead of plugging into
//! luma's own picking backend we emit ordinary `bevy_picking` `PointerHits`, so `bevy_ui`
//! widgets and `bevy_picking` observers work unchanged.

use bevy::picking::PickingSystems;
use bevy::picking::backend::ray::RayMap;
use bevy::picking::Pickable;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::prelude::*;
use bevy_ui::{ComputedNode, Node};

use crate::util::offset_from_panel;
use crate::world3d::{MirrorRoot, WorldPanel};

/// One pickable rectangle, in panel-local physical pixels (centre + half size).
#[derive(Clone, Copy, Debug)]
pub struct PickRect {
    pub entity: Entity,
    pub panel: Entity,
    pub center: Vec2,
    pub half: Vec2,
    /// Painter order within the panel (higher = drawn later = on top).
    pub order: u32,
}

/// Rebuilt every frame in `PostUpdate` (layout is final there).
#[derive(Resource, Default)]
pub struct PanelPickIndex {
    pub rects: Vec<PickRect>,
}

/// Collects every filled panel node's rectangle.
#[allow(clippy::too_many_arguments)]
pub fn build_pick_index(
    mut index: ResMut<PanelPickIndex>,
    panels: Query<Entity, With<MirrorRoot>>,
    // 只有显式标了 `Pickable` 的节点参与拾取：面板背景别抢行的点击。
    nodes: Query<
        (
            Entity,
            &ComputedNode,
            &bevy_ui::UiGlobalTransform,
            Option<&bevy_ui::BackgroundColor>,
        ),
        (With<Node>, With<Pickable>),
    >,
    all_nodes: Query<&bevy_ui::UiGlobalTransform>,
    parents: Query<&ChildOf>,
) {
    index.rects.clear();
    let mut order = 0u32;
    for panel in panels.iter() {
        for (entity, cn, ugt, bg) in nodes.iter() {
            let Some(offset) = offset_from_panel(entity, panel, ugt, &all_nodes, &parents) else {
                continue;
            };
            // `Pickable` alone makes a node a target: a row without a band still
            // needs to be clickable (selection, tool cards).
            let _ = bg;
            index.rects.push(PickRect {
                entity,
                panel,
                center: offset,
                half: cn.size * 0.5,
                order,
            });
            order += 1;
        }
    }
}

/// Ray -> panel plane -> node rects -> `PointerHits`.
pub fn panel_picking(
    ray_map: Res<RayMap>,
    index: Res<PanelPickIndex>,
    panels: Query<&WorldPanel, With<MirrorRoot>>,
    mut output: MessageWriter<PointerHits>,
) {
    if index.rects.is_empty() {
        return;
    }
    for (ray_id, ray) in ray_map.iter() {
        let mut picks: Vec<(Entity, HitData, u32)> = Vec::new();
        for rect in index.rects.iter() {
            let Ok(panel) = panels.get(rect.panel) else {
                continue;
            };
            let anchor = panel.anchor;
            let normal = (anchor.rotation * Vec3::Z).normalize();
            let denominator = ray.direction.dot(normal);
            if denominator.abs() < 1e-6 {
                continue;
            }
            let t = (anchor.translation - ray.origin).dot(normal) / denominator;
            if t < 0.0 {
                continue;
            }
            let hit = ray.origin + t * ray.direction;
            let local = anchor.to_matrix().inverse().transform_point3(hit);
            // world metres -> panel-local physical px (UI y grows downwards)
            let px = Vec2::new(
                local.x / panel.units_to_world,
                -local.y / panel.units_to_world,
            );
            let d = px - rect.center;
            if d.x.abs() <= rect.half.x && d.y.abs() <= rect.half.y {
                picks.push((
                    rect.entity,
                    HitData::new(ray_id.camera, t, Some(hit), Some(normal)),
                    rect.order,
                ));
            }
        }
        if picks.is_empty() {
            continue;
        }
        // Coplanar nodes have the same depth: break ties by painter order, so the
        // child (row band) wins over the panel background it sits on.
        picks.sort_by(|a, b| {
            a.1.depth
                .total_cmp(&b.1.depth)
                .then(b.2.cmp(&a.2))
        });
        let picks = picks
            .into_iter()
            .map(|(entity, hit, _)| (entity, hit))
            .collect();
        output.write(PointerHits::new(ray_id.pointer, picks, 0.0));
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<PanelPickIndex>()
        .add_systems(PostUpdate, build_pick_index)
        .add_systems(PreUpdate, panel_picking.in_set(PickingSystems::Backend));
}
