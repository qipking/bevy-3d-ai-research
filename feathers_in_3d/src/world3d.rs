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

/// Extra local Z per hierarchy level, in world units.
pub const Z_BIAS_PER_LEVEL: f32 = 0.0002;

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
    panels: Query<(Entity, &WorldPanel), With<MirrorRoot>>,
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
    let panel_map: HashMap<Entity, (f32, Transform)> = panels
        .iter()
        .map(|(e, p)| (e, (p.units_to_world, p.anchor)))
        .collect();
    if panel_map.is_empty() {
        return;
    }

    let mut in_panel: HashMap<Entity, (Entity, u32)> = HashMap::new();
    for root in panel_map.keys() {
        let mut stack = vec![(*root, 0u32)];
        while let Some((e, depth)) = stack.pop() {
            in_panel.insert(e, (*root, depth));
            if let Ok(kids) = children.get(e) {
                for kid in kids.iter() {
                    stack.push((kid, depth + 1));
                }
            }
        }
    }

    for (node, (panel_root, depth)) in in_panel.iter() {
        let Some((units_to_world, anchor)) = panel_map.get(panel_root).copied() else {
            continue;
        };
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
        commands.entity(entity).insert(placed);
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
            let material = UNodeMaterial3d {
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
