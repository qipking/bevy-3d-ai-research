//! 3D text: `bevy_text`'s laid-out glyphs -> one textured quad mesh per text node.
//!
//! Positions come from `TextLayoutInfo`, uvs from `GlyphAtlasInfo.rect`, colors
//! from each glyph's section `TextColor` (as vertex colors). One `StandardMaterial`
//! per font atlas, so styled spans (syntax highlight, diff colors) work too.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::text::{ComputedTextBlock, TextLayoutInfo};
use bevy_ui::{ComputedNode, Node, UiGlobalTransform};

use crate::world3d::{MirrorRoot, WorldPanel, Z_BIAS_PER_LEVEL};

#[derive(Resource, Default)]
pub struct GlyphMaterials(pub HashMap<AssetId<Image>, Handle<StandardMaterial>>);

#[derive(Component)]
pub struct Text3d {
    pub mesh: Handle<Mesh>,
    pub glyphs: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn sync_text3d(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut glyph_materials: ResMut<GlyphMaterials>,
    panels: Query<(Entity, &WorldPanel), With<MirrorRoot>>,
    texts: Query<
        (
            Entity,
            &TextLayoutInfo,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&ComputedTextBlock>,
            Option<&TextColor>,
        ),
        With<Text>,
    >,
    text_colors: Query<&TextColor>,
    parents: Query<&ChildOf>,
    all_nodes: Query<(Entity, &UiGlobalTransform)>,
    existing: Query<&Text3d>,
) {
    let panel_map: HashMap<Entity, f32> = panels
        .iter()
        .map(|(e, p)| (e, p.units_to_world))
        .collect();
    if panel_map.is_empty() {
        return;
    }

    for (entity, info, cn, ugt, block, text_color) in texts.iter() {
        if info.glyphs.is_empty() {
            continue;
        }
        if let Ok(old) = existing.get(entity)
            && old.glyphs == info.glyphs.len()
        {
            continue;
        }

        let mut current = entity;
        let mut depth = 0u32;
        let mut panel: Option<f32> = None;
        loop {
            if let Some(u) = panel_map.get(&current) {
                panel = Some(*u);
                break;
            }
            let Ok(child_of) = parents.get(current) else { break };
            current = child_of.parent();
            depth += 1;
            if depth > 64 {
                break;
            }
        }
        let Some(units_to_world) = panel else { continue };

        let parent_centre_offset = match parents.get(entity) {
            Ok(child_of) => match all_nodes.get(child_of.parent()) {
                Ok((_, pugt)) => ugt.affine().translation - pugt.affine().translation,
                Err(_) => Vec2::ZERO,
            },
            Err(_) => Vec2::ZERO,
        };

        let content_min = cn.content_box().min;
        let base_color = text_color
            .map(|c| LinearRgba::from(c.0).to_f32_array())
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);

        // Per-glyph color from its section's `TextColor`.
        let section_color = |section: u32| -> [f32; 4] {
            block
                .and_then(|b| b.entities().get(section as usize))
                .and_then(|t| text_colors.get(t.entity).ok())
                .map(|c| LinearRgba::from(c.0).to_f32_array())
                .unwrap_or(base_color)
        };

        let mut by_atlas: HashMap<AssetId<Image>, Vec<(Vec2, Rect, [f32; 4])>> = HashMap::new();
        for glyph in info.glyphs.iter() {
            by_atlas.entry(glyph.atlas_info.texture).or_default().push((
                glyph.position,
                glyph.atlas_info.rect,
                section_color(glyph.section_index),
            ));
        }

        for (atlas_id, glyphs) in by_atlas {
            let Some(texture_size) = images.get(atlas_id).map(|img| img.size().as_vec2()) else {
                continue;
            };
            let Some(texture_handle) = images.get_strong_handle(atlas_id) else {
                continue;
            };

            let mut positions: Vec<[f32; 3]> = Vec::with_capacity(glyphs.len() * 4);
            let mut normals: Vec<[f32; 3]> = Vec::with_capacity(glyphs.len() * 4);
            let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(glyphs.len() * 4);
            let mut colors: Vec<[f32; 4]> = Vec::with_capacity(glyphs.len() * 4);
            let mut indices: Vec<u32> = Vec::with_capacity(glyphs.len() * 6);

            for (position, rect, color) in glyphs.iter() {
                let centre = content_min + *position;
                let half = rect.size() * 0.5 * units_to_world;
                let cx = centre.x * units_to_world;
                let cy = -centre.y * units_to_world;

                let base = positions.len() as u32;
                positions.extend_from_slice(&[
                    [cx - half.x, cy - half.y, 0.0],
                    [cx + half.x, cy - half.y, 0.0],
                    [cx + half.x, cy + half.y, 0.0],
                    [cx - half.x, cy + half.y, 0.0],
                ]);
                normals.extend_from_slice(&[[0.0, 0.0, 1.0]; 4]);
                colors.extend_from_slice(&[*color; 4]);

                let uv_min = rect.min / texture_size;
                let uv_max = rect.max / texture_size;
                uvs.extend_from_slice(&[
                    [uv_min.x, uv_max.y],
                    [uv_max.x, uv_max.y],
                    [uv_max.x, uv_min.y],
                    [uv_min.x, uv_min.y],
                ]);
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }

            let mut mesh = Mesh::new(
                bevy::render::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(bevy::render::mesh::Indices::U32(indices));

            let material = glyph_materials
                .0
                .entry(atlas_id)
                .or_insert_with(|| {
                    std_materials.add(StandardMaterial {
                        base_color_texture: Some(texture_handle.clone()),
                        base_color: Color::WHITE,
                        unlit: true,
                        alpha_mode: AlphaMode::Blend,
                        cull_mode: None,
                        ..default()
                    })
                })
                .clone();

            let mesh = meshes.add(mesh);
            let transform = Transform::from_translation(Vec3::new(
                parent_centre_offset.x * units_to_world,
                -parent_centre_offset.y * units_to_world,
                depth as f32 * Z_BIAS_PER_LEVEL,
            ));

            commands.entity(entity).insert((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                transform,
                Text3d { mesh: mesh.clone(), glyphs: glyphs.len() },
            ));
        }
    }
}

// `Node` is only imported so the module keeps the same query bounds as the mirror.
const _: fn() = || {
    fn needs_node(_: &Node) {}
};
