//! 3D text: `bevy_text`'s laid-out glyphs -> **one batched quad mesh per (panel, font atlas)**.
//!
//! Why batched per panel rather than per text node:
//! * a transcript panel has many rows; one mesh per row would be one draw call per row;
//! * a single line can mix fonts (Latin + CJK fallback) and therefore atlases — a per-node
//!   builder would have to overwrite its own mesh when the second atlas shows up.
//!
//! Glyph positions come from `TextLayoutInfo`, uvs from `GlyphAtlasInfo.rect`, colors from
//! each glyph's section `TextColor` (as vertex colors), so styled spans keep working.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy::text::{ComputedTextBlock, TextLayoutInfo};
use bevy_ui::{ComputedNode, Node, UiGlobalTransform};

use crate::sort::PanelSort;
use crate::world3d::{LAYER_BIAS, MirrorRoot, PanelSortRange, WorldLayer, WorldPanel, Z_BIAS_PER_LEVEL};

/// Cache of one `StandardMaterial` per font-atlas texture.
#[derive(Resource, Default)]
pub struct GlyphMaterials(pub HashMap<AssetId<Image>, Handle<StandardMaterial>>);

/// Set whenever laid-out text changed, so panels rebuild their glyph meshes.
#[derive(Resource, Default)]
pub struct TextDirty(pub bool);

/// Glyph meshes owned by one panel: one child entity per atlas.
#[derive(Component, Default)]
pub struct PanelTextMeshes {
    pub children: Vec<Entity>,
    pub signature: u64,
}

/// Marks a rebuilt-on-demand glyph mesh child (so a rebuild can despawn them).
#[derive(Component)]
pub struct GlyphMeshChild;

pub fn mark_text_dirty(
    added_or_changed: Query<(), Changed<TextLayoutInfo>>,
    mut removed: RemovedComponents<TextLayoutInfo>,
    mut dirty: ResMut<TextDirty>,
) {
    if removed.read().count() > 0 || !added_or_changed.is_empty() {
        dirty.0 = true;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn sync_panel_text(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut glyph_materials: ResMut<GlyphMaterials>,
    mut dirty: ResMut<TextDirty>,
    mut panels: Query<
        (
            Entity,
            &WorldPanel,
            &ComputedNode,
            Option<&WorldLayer>,
            Option<&PanelSortRange>,
            Option<&mut PanelTextMeshes>,
        ),
        With<MirrorRoot>,
    >,
    texts: Query<(
        Entity,
        &TextLayoutInfo,
        &ComputedNode,
        &UiGlobalTransform,
        Option<&ComputedTextBlock>,
        Option<&TextColor>,
    )>,
    all_nodes: Query<&UiGlobalTransform>,
    text_colors: Query<&TextColor>,
    parents: Query<&ChildOf>,
) {
    if panels.is_empty() {
        return;
    }
    // A system that changed nothing still needs one rebuild after startup.
    let mut any_rebuild = dirty.0;
    dirty.0 = false;

    for (panel_entity, panel, panel_cn, layer, bias_range, existing) in panels.iter_mut() {
        let units = panel.units_to_world;
        let layer_bias = layer.map_or(0, |layer| layer.0) as f32 * LAYER_BIAS;

        // Collect this panel's text nodes and their offsets from the panel centre.
        let clip_half = panel_cn.size * 0.5;
        let mut rows: Vec<PanelText> = Vec::new();
        for (entity, info, cn, ugt, block, color) in texts.iter() {
            if info.glyphs.is_empty() {
                continue;
            }
            let Some(offset) = offset_from_panel(
                entity, panel_entity, ugt, &all_nodes, &parents,
            ) else {
                continue;
            };
            rows.push(PanelText {
                info,
                cn,
                block,
                color,
                offset,
                entity,
            });
        }

        // Cheap signature: rebuild when anything that changes the quads changed.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        panel_cn.size.to_bits_parts().hash(&mut hasher);
        for row in &rows {
            row.entity.hash(&mut hasher);
            row.info.glyphs.len().hash(&mut hasher);
            row.offset.x.to_bits().hash(&mut hasher);
            row.offset.y.to_bits().hash(&mut hasher);
            if let Some(first) = row.info.glyphs.first() {
                first.position.x.to_bits().hash(&mut hasher);
                first.position.y.to_bits().hash(&mut hasher);
            }
        }
        let signature = hasher.finish();

        if let Some(meshes_comp) = &existing
            && meshes_comp.signature == signature
            && !any_rebuild
        {
            continue;
        }
        any_rebuild = false;

        // Group every glyph of the panel by atlas.
        let mut by_atlas: HashMap<AssetId<Image>, Vec<[f32; 4]>> = HashMap::new();
        let mut positions: HashMap<AssetId<Image>, Vec<[f32; 3]>> = HashMap::new();
        let mut normals: HashMap<AssetId<Image>, Vec<[f32; 3]>> = HashMap::new();
        let mut uvs: HashMap<AssetId<Image>, Vec<[f32; 2]>> = HashMap::new();
        let mut indices: HashMap<AssetId<Image>, Vec<u32>> = HashMap::new();

        for row in &rows {
            let content_min = row.cn.content_box().min;
            let base_color = row
                .color
                .map(|c| LinearRgba::from(c.0).to_f32_array())
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let section_color = |section: u32| -> [f32; 4] {
                row.block
                    .and_then(|b| b.entities().get(section as usize))
                    .and_then(|t| text_colors.get(t.entity).ok())
                    .map(|c| LinearRgba::from(c.0).to_f32_array())
                    .unwrap_or(base_color)
            };

            for glyph in row.info.glyphs.iter() {
                let atlas = glyph.atlas_info.texture;
                let texture_size = match images.get(atlas) {
                    Some(image) => image.size().as_vec2(),
                    None => continue,
                };
                let centre = row.offset + content_min + glyph.position;
                // Clip to the panel box (one glyph of margin keeps half-scrolled rows sane).
                let margin = 2.0;
                if centre.x < -clip_half.x - margin
                    || centre.x > clip_half.x + margin
                    || centre.y < -clip_half.y - margin
                    || centre.y > clip_half.y + margin
                {
                    continue;
                }
                let half = glyph.atlas_info.rect.size() * 0.5 * units;
                let cx = centre.x * units;
                let cy = -centre.y * units;

                let pos = positions.entry(atlas).or_default();
                let nrm = normals.entry(atlas).or_default();
                let uv = uvs.entry(atlas).or_default();
                let col = by_atlas.entry(atlas).or_default();
                let idx = indices.entry(atlas).or_default();

                let base = pos.len() as u32;
                pos.extend_from_slice(&[
                    [cx - half.x, cy - half.y, 0.0],
                    [cx + half.x, cy - half.y, 0.0],
                    [cx + half.x, cy + half.y, 0.0],
                    [cx - half.x, cy + half.y, 0.0],
                ]);
                nrm.extend_from_slice(&[[0.0, 0.0, 1.0]; 4]);
                col.extend_from_slice(&[section_color(glyph.section_index); 4]);

                let uv_min = glyph.atlas_info.rect.min / texture_size;
                let uv_max = glyph.atlas_info.rect.max / texture_size;
                uv.extend_from_slice(&[
                    [uv_min.x, uv_max.y],
                    [uv_max.x, uv_max.y],
                    [uv_max.x, uv_min.y],
                    [uv_min.x, uv_min.y],
                ]);
                idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }

        // Drop the previous meshes, then spawn one child per atlas.
        if let Some(meshes_comp) = &existing {
            for child in &meshes_comp.children {
                commands.entity(*child).despawn();
            }
        }

        let by_atlas_count = positions.len();
        let mut children = Vec::new();
        for (atlas, pos) in positions {
            let Some(texture_handle) = images.get_strong_handle(atlas) else {
                continue;
            };
            let mut mesh = Mesh::new(
                bevy::render::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.remove(&atlas).unwrap());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs.remove(&atlas).unwrap());
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, by_atlas.remove(&atlas).unwrap());
            mesh.insert_indices(bevy::render::mesh::Indices::U32(
                indices.remove(&atlas).unwrap(),
            ));

            let material = glyph_materials
                .0
                .entry(atlas)
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

            let child = commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material),
                    Transform::from_translation(Vec3::new(0.0, 0.0, Z_BIAS_PER_LEVEL * 0.5)),
                    ChildOf(panel_entity),
                    GlyphMeshChild,
                    PanelSort {
                        center: panel.anchor.translation,
                        bias: layer_bias
                            + bias_range.map_or(0.02, |r| r.max_bias)
                            + 0.0002 * children.len() as f32,
                    },
                ))
                .id();
            children.push(child);
        }

        info!(
            "[text3d] panel={panel_entity:?} rows={} glyphs={} atlases={} draw_calls={}",
            rows.len(),
            rows.iter().map(|r| r.info.glyphs.len()).sum::<usize>(),
            by_atlas_count,
            children.len(),
        );
        commands.entity(panel_entity).insert(PanelTextMeshes {
            children,
            signature,
        });
    }
}

struct PanelText<'a> {
    entity: Entity,
    info: &'a TextLayoutInfo,
    cn: &'a ComputedNode,
    block: Option<&'a ComputedTextBlock>,
    color: Option<&'a TextColor>,
    /// Text-node centre relative to the panel centre, in physical pixels.
    offset: Vec2,
}

/// Sums node-centre offsets from `panel` down to `leaf`.
fn offset_from_panel(
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
        // Add *this* hop before the stop test: the last hop (row -> panel) carries
        // the row's own offset, and dropping it collapses every row onto the panel centre.
        offset += current_ugt.affine().translation - parent_ugt.affine().translation;
        if parent == panel {
            return Some(offset);
        }
        current = parent;
        current_ugt = parent_ugt;
    }
    None
}

trait BitsParts {
    fn to_bits_parts(&self) -> (u32, u32);
}
impl BitsParts for Vec2 {
    fn to_bits_parts(&self) -> (u32, u32) {
        (self.x.to_bits(), self.y.to_bits())
    }
}

// `Node` keeps the query bounds aligned with the mirror's.
#[allow(dead_code)]
fn needs_node(_: &Node) {}
