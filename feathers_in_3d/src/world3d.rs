//! Route B: lay `bevy_ui`/feathers nodes out in screen space, mirror the result
//! into world-space meshes with luma's SDF material (see VERIFICATION_REPORT.md).
//!
//! Only geometry/background/border/radius are mirrored here; text is drawn by
//! `text3d` from `bevy_text`'s laid-out glyphs.

use std::collections::HashMap;

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d;
use bevy_ui::{BackgroundColor, BorderColor, ComputedNode, Node, UiGlobalTransform};

use crate::sort::{PanelSort, SORT_STEP};

/// The painter bias a panel's nodes reach; text meshes are placed just above it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PanelSortRange {
    pub max_bias: f32,
    /// The [`WorldLayer`] the range was measured on.
    pub layer: u16,
}

/// Extra local Z per hierarchy level, in world units.
pub const Z_BIAS_PER_LEVEL: f32 = 0.0;

/// How much one [`WorldLayer`] step adds to the painter bias.
///
/// The bias is a *sort key*, not geometry: a panel on layer 1 always draws over
/// everything on layer 0, at any camera angle, however many nodes layer 0 has.
/// That is what a UI wants for overlays (a picker over the transcript); sorting
/// coplanar quads by their mesh centre alone cannot express it.
pub const LAYER_BIAS: f32 = 1.0;

/// Painter layer of a whole panel: `0` is the screen's own layout, `1` a box
/// drawn over it, and so on. Every node and glyph of the panel inherits it.
/// A frame model sets it from its placements.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorldLayer(pub u16);

/// Marks a `bevy_ui` subtree that should also exist in the 3D world.
#[derive(Component, Clone, Copy, Debug)]
pub struct WorldPanel {
    pub units_to_world: f32,
    pub anchor: Transform,
}

#[derive(Component)]
pub struct MirrorRoot;

#[derive(Component)]
pub struct PanelNode {
    pub mesh: Handle<Mesh>,
    pub material: Handle<UNodeMaterial3d>,
    pub depth: u32,
    pub cached_size: Vec2,
    pub cached_color: Vec4,
    pub cached_border: Vec4,
    pub cached_radius: Vec4,
    pub cached_border_width: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn sync_mirror(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
    panels: Query<(Entity, &WorldPanel, Option<&WorldLayer>), With<MirrorRoot>>,
    mut existing: Query<&mut PanelNode>,
    all_nodes: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&BackgroundColor>,
            Option<&BorderColor>,
        ),
        With<Node>,
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
) {
    let panel_map: HashMap<Entity, (f32, Transform, u16)> = panels
        .iter()
        .map(|(e, p, layer)| (e, (p.units_to_world, p.anchor, layer.map_or(0, |l| l.0))))
        .collect();
    if panel_map.is_empty() {
        return;
    }

    // Per-panel clip: UI subtree content is clipped to the panel's own box, which is what a
    // transcript needs once it scrolls. The material clips in *panel-local* space, so the
    // uniform rows are the rows of the panel's world -> local matrix.
    let mut panel_clip: HashMap<Entity, (Vec4, Vec4, Vec2)> = HashMap::new();
    for (root, _, _) in panels.iter() {
        let Ok((_, cn, _, _, _)) = all_nodes.get(root) else {
            continue;
        };
        let (units, anchor, _) = panel_map
            .get(&root)
            .copied()
            .unwrap_or((0.001, Transform::default(), 0));
        let inv = anchor.to_matrix().inverse();
        let m = inv.to_cols_array_2d();
        let row0 = Vec4::new(m[0][0], m[1][0], m[2][0], m[3][0]);
        let row1 = Vec4::new(m[0][1], m[1][1], m[2][1], m[3][1]);
        // bounds are in the *clip space* the shader works in: metres, not UI pixels
        panel_clip.insert(root, (row0, row1, cn.size * 0.5 * units));
    }

    // Depth-first pre-order gives every node a painter index: a node always comes
    // before its children, so children draw on top of their parent.
    let mut in_panel: HashMap<Entity, (Entity, u32, u32)> = HashMap::new();
    let mut panel_max_bias: HashMap<Entity, f32> = HashMap::new();
    for root in panel_map.keys() {
        let mut stack = vec![(*root, 0u32)];
        let mut order = 0u32;
        while let Some((e, depth)) = stack.pop() {
            in_panel.insert(e, (*root, depth, order));
            order += 1;
            if let Ok(kids) = children.get(e) {
                for kid in kids.iter().rev() {
                    stack.push((kid, depth + 1));
                }
            }
        }
        let layer = panel_map.get(root).map_or(0, |(_, _, layer)| *layer);
        let layer_bias = layer as f32 * LAYER_BIAS;
        panel_max_bias.insert(*root, layer_bias + order.saturating_sub(1) as f32 * SORT_STEP);
    }
    for (root, max_bias) in panel_max_bias {
        let layer = panel_map.get(&root).map_or(0, |(_, _, layer)| *layer);
        commands.entity(root).insert(PanelSortRange { max_bias, layer });
    }

    for (node, (panel_root, depth, order)) in in_panel.iter() {
        let Some((units_to_world, anchor, layer)) = panel_map.get(panel_root).copied() else {
            continue;
        };
        let layer_bias = layer as f32 * LAYER_BIAS;
        let Ok((entity, cn, ugt, bg, border_color)) = all_nodes.get(*node) else {
            continue;
        };

        let border_px = cn
            .border
            .min_inset
            .x
            .max(cn.border.min_inset.y)
            .max(cn.border.max_inset.x)
            .max(cn.border.max_inset.y);
        let has_fill = bg.is_some_and(|c| c.0.alpha() > 0.0) || border_px > 0.0;

        let size_world = cn.size * units_to_world;

        let parent_centre_offset = match parents.get(entity) {
            Ok(child_of) => match all_nodes.get(child_of.parent()) {
                Ok((_, _, pugt, _, _)) => {
                    ugt.affine().translation - pugt.affine().translation
                }
                Err(_) => Vec2::ZERO,
            },
            Err(_) => Vec2::ZERO,
        };

        let local_pos = Vec3::new(
            parent_centre_offset.x * units_to_world,
            -parent_centre_offset.y * units_to_world,
            *depth as f32 * Z_BIAS_PER_LEVEL,
        );

        // EVERY node in a panel gets a world Transform, even without a fill:
        // text children hang off their row node, so an unplaced row would
        // collapse all of its text onto the panel origin.
        let placed = if *depth == 0 {
            anchor
        } else {
            Transform::from_translation(local_pos)
        };
        commands.entity(entity).insert((
            placed,
            PanelSort {
                center: anchor.translation,
                bias: layer_bias + *order as f32 * SORT_STEP,
            },
        ));
        if !has_fill {
            continue;
        }

        let base_color = bg
            .map(|c| LinearRgba::from(c.0))
            .unwrap_or(LinearRgba::NONE);
        let b_color = border_color
            .map(|c| LinearRgba::from(c.top))
            .unwrap_or(LinearRgba::NONE);
        let b_width = border_px * units_to_world;

        let r = &cn.border_radius;
        let radius_vec = Vec4::new(
            r.top_right.x.max(r.top_right.y) * units_to_world,
            r.bottom_right.x.max(r.bottom_right.y) * units_to_world,
            r.top_left.x.max(r.top_left.y) * units_to_world,
            r.bottom_left.x.max(r.bottom_left.y) * units_to_world,
        );

        let color4 = base_color.to_vec4();
        let border4 = b_color.to_vec4();

        if let Ok(mut node3d) = existing.get_mut(entity) {
            if node3d.cached_size != size_world {
                let mesh = meshes.add(Rectangle::new(size_world.x, size_world.y));
                node3d.mesh = mesh.clone();
                node3d.cached_size = size_world;
                commands.entity(entity).insert(Mesh3d(mesh));
            }
            node3d.depth = *depth;
            if node3d.cached_color != color4
                || node3d.cached_border != border4
                || node3d.cached_radius != radius_vec
                || !(node3d.cached_border_width - b_width).abs().lt(&1e-9)
            {
                if let Some(mut mat) = materials.get_mut(&node3d.material) {
                    if let Some((row0, row1, half)) = panel_clip.get(panel_root) {
                        mat.clip_inv_x = [*row0, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO];
                        mat.clip_inv_y = [*row1, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO];
                        mat.clip_bounds = [
                            Vec4::new(half.x, half.y, 1.0, 0.0),
                            Vec4::ZERO,
                            Vec4::ZERO,
                            Vec4::ZERO,
                        ];
                    }
                    mat.color = color4;
                    mat.border_color = border4;
                    mat.radius = radius_vec;
                    mat.size = size_world;
                    mat.border_width = b_width;
                    mat.softness = 1.0e-5;
                    node3d.cached_color = color4;
                    node3d.cached_border = border4;
                    node3d.cached_radius = radius_vec;
                    node3d.cached_border_width = b_width;
                }
            }
        } else {
            let (clip_row0, clip_row1, clip_half) = panel_clip
                .get(panel_root)
                .copied()
                .unwrap_or((Vec4::ZERO, Vec4::ZERO, Vec2::ZERO));
            let material = UNodeMaterial3d {
                clip_inv_x: [clip_row0, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO],
                clip_inv_y: [clip_row1, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO],
                clip_bounds: [
                    Vec4::new(clip_half.x, clip_half.y, 1.0, 0.0),
                    Vec4::ZERO,
                    Vec4::ZERO,
                    Vec4::ZERO,
                ],
                color: color4,
                radius: radius_vec,
                border_color: border4,
                size: size_world,
                border_width: b_width,
                softness: 1.0e-5,
                roughness: 0.55,
                ..default()
            };
            let mesh = meshes.add(Rectangle::new(size_world.x, size_world.y));
            let material = materials.add(material);
            commands.entity(entity).insert((
                PanelNode {
                    mesh: mesh.clone(),
                    material: material.clone(),
                    depth: *depth,
                    cached_size: size_world,
                    cached_color: color4,
                    cached_border: border4,
                    cached_radius: radius_vec,
                    cached_border_width: b_width,
                },
                Mesh3d(mesh),
                MeshMaterial3d(material),
            ));
        }
    }
}
