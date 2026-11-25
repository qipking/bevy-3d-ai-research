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
use crate::world3d::{
    LAYER_BIAS, MirrorRoot, PanelSortRange, WorldLayer, WorldPanel, Z_BIAS_PER_LEVEL,
    effective_clip, panel_world_rect, physical_size_to_world, physical_to_logical,
};

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
        let inverse_scale_factor = panel_cn.inverse_scale_factor;
        let layer_bias = layer.map_or(0, |layer| layer.0) as f32 * LAYER_BIAS;

        // Collect this panel's text nodes and their offsets from the panel centre.
        // `ComputedNode`/`UiGlobalTransform` 都是物理像素，镜像统一用逻辑像素
        // （见 `world3d` 的换算说明）；1× 下 inverse_scale_factor == 1.0，结果不变。
        //
        // 文字裁剪和材质用同一口径：面板自身矩形 ∩ `WorldPanel::clip`。`None` 或
        // 视口完整包住面板时就是面板自身（scroll = 0 的常规情况，与旧行为一致）；
        // 交集为空（面板整个滚出视口）时一个字都不画。
        let clip_bounds = text_clip_bounds(panel, panel_cn, inverse_scale_factor);
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
        inverse_scale_factor.to_bits().hash(&mut hasher);
        match clip_bounds {
            Some((min_x, max_x, min_y, max_y)) => {
                1u8.hash(&mut hasher);
                min_x.to_bits().hash(&mut hasher);
                max_x.to_bits().hash(&mut hasher);
                min_y.to_bits().hash(&mut hasher);
                max_y.to_bits().hash(&mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
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
                let centre = physical_to_logical(
                    row.offset + content_min + glyph.position,
                    inverse_scale_factor,
                );
                // Clip to the panel box ∩ strip viewport (one glyph of margin keeps
                // half-scrolled rows sane). An empty intersection culls everything.
                let margin = 2.0;
                let Some((min_x, max_x, min_y, max_y)) = clip_bounds else {
                    continue;
                };
                if centre.x < min_x - margin
                    || centre.x > max_x + margin
                    || centre.y < min_y - margin
                    || centre.y > max_y + margin
                {
                    continue;
                }
                let half = physical_size_to_world(
                    glyph.atlas_info.rect.size() * 0.5,
                    inverse_scale_factor,
                    units,
                );
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

/// 文字可见区：`面板自身 ∩ WorldPanel::clip`，换算成"面板中心为原点、
/// y 朝下"的**逻辑像素**边界 `(min_x, max_x, min_y, max_y)`。
///
/// 返回 `None` = 交集为空（面板整个在视口外，或者本身没有面积）：一个字形都
/// 不该画。`clip = None` 或视口完整包住面板时退回面板自身——和加 clip 之前
/// 的 `±panel_half` 逐字节相同（1× 缩放下）。
fn text_clip_bounds(
    panel: &WorldPanel,
    panel_cn: &ComputedNode,
    inverse_scale_factor: f32,
) -> Option<(f32, f32, f32, f32)> {
    let half = physical_to_logical(panel_cn.size * 0.5, inverse_scale_factor);
    let panel_bounds = (-half.x, half.x, -half.y, half.y);
    let Some(clip) = panel.clip else {
        return Some(panel_bounds);
    };
    let units = panel.units_to_world;
    let size_world = physical_size_to_world(panel_cn.size, inverse_scale_factor, units);
    let panel_rect = panel_world_rect(&panel.anchor, size_world);
    // 视口完整包住面板：交集就是面板自身，用旧口径（逐字节不变）。
    if clip.contains(panel_rect.min) && clip.contains(panel_rect.max) {
        return Some(panel_bounds);
    }
    let effective = effective_clip(panel_rect, Some(clip));
    if effective.is_empty() || units <= 0.0 {
        return None;
    }
    // 把有效裁剪矩形（世界坐标）的四个角变换回面板局部（米），取 AABB；
    // 锚点带旋转时这才是正确的局部可见区。世界 y 朝上 → 文字逻辑 y 朝下。
    let inv = panel.anchor.to_matrix().inverse();
    let inv_units = units.recip();
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for corner in [
        effective.min,
        Vec2::new(effective.max.x, effective.min.y),
        effective.max,
        Vec2::new(effective.min.x, effective.max.y),
    ] {
        let local = inv.transform_point3(corner.extend(0.0));
        let local = Vec2::new(local.x, -local.y) * inv_units;
        min = min.min(local);
        max = max.max(local);
    }
    Some((min.x, max.x, min.y, max.y))
}

struct PanelText<'a> {
    entity: Entity,
    info: &'a TextLayoutInfo,
    cn: &'a ComputedNode,
    block: Option<&'a ComputedTextBlock>,
    color: Option<&'a TextColor>,
    /// Text-node centre relative to the panel centre, in physical pixels
    /// (the mirror converts it to logical pixels before building glyphs).
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 文字可见区 = 面板 ∩ 条带视口，换算成面板中心为原点、y 朝下的逻辑像素；
    /// 面板整个在视口外时返回 `None`（一个字都不画）。
    #[test]
    fn text_clip_bounds_follow_the_strip_viewport() {
        let mut panel = WorldPanel {
            units_to_world: 0.0018,
            anchor: Transform::default(),
            clip: None,
        };
        let cn = ComputedNode {
            size: Vec2::new(200.0, 100.0),
            ..default()
        };
        // 没有 clip：面板自身（和加 clip 之前的 ±panel_half 完全一致）。
        let full = text_clip_bounds(&panel, &cn, 1.0).expect("有面积");
        assert!((full.0 + 100.0).abs() < 1e-6 && (full.1 - 100.0).abs() < 1e-6);
        assert!((full.2 + 50.0).abs() < 1e-6 && (full.3 - 50.0).abs() < 1e-6);

        // 视口只留右半边（世界 x >= 0）：逻辑 x ∈ [0, 100]。
        panel.clip = Some(Rect::new(0.0, -1.0, 1.0, 1.0));
        let right_half = text_clip_bounds(&panel, &cn, 1.0).expect("右半边可见");
        assert!(right_half.0.abs() < 1e-3, "左边界 = 面板中心");
        assert!((right_half.1 - 100.0).abs() < 1e-3, "右边界 = 面板右边界");
        assert!((right_half.2 + 50.0).abs() < 1e-3 && (right_half.3 - 50.0).abs() < 1e-3);

        // 视口完全在面板外：None（不画任何字形）。
        panel.clip = Some(Rect::new(5.0, 5.0, 6.0, 6.0));
        assert!(
            text_clip_bounds(&panel, &cn, 1.0).is_none(),
            "面板整个滚出视口：文字一个都不画"
        );
    }
}
