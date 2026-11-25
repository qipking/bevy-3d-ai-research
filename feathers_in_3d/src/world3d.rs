//! Route B: lay `bevy_ui`/feathers nodes out in screen space, mirror the result
//! into world-space meshes with luma's SDF material (see VERIFICATION_REPORT.md).
//!
//! Only geometry/background/border/radius are mirrored here; text is drawn by
//! `text3d` from `bevy_text`'s laid-out glyphs.

use std::collections::HashMap;

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d;
use bevy_ui::{
    BackgroundColor, BorderColor, ComputedNode, Node, Outline, ResolvedBorderRadius,
    UiGlobalTransform,
};

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
    /// **逻辑** UI 像素 → 世界米。
    ///
    /// `ComputedNode` 给的是物理像素；镜像前会先乘节点的
    /// `inverse_scale_factor` 回到逻辑像素（见本文件上方的换算说明）。
    pub units_to_world: f32,
    pub anchor: Transform,
    /// 额外的**世界坐标、轴对齐**裁剪矩形（工作区条带的视口）。
    ///
    /// `None` = 不额外裁剪（面板自身矩形就是全部）；`Some(rect)` 时镜像的
    /// 材质/文字都只画 `面板自身矩形 ∩ rect` 里的部分。面板**不**会因为完全
    /// 在 `rect` 外而被 despawn：交集为空时材质宽/高为 0（shader 丢弃所有片元，
    /// 不会 panic），实体还在，滚回来就能重新看见。
    pub clip: Option<Rect>,
}

/// 面板在世界里的**轴对齐**包围盒（`anchor` 是面板中心，`size` 是世界米尺寸）。
///
/// 工作区通常只平移（轴对齐 = 面板自身矩形）；锚点带旋转/缩放时取真实的
/// 世界 AABB，裁剪只可能多留一些空角，不会把面板还看得见的部分误裁掉。
pub fn panel_world_rect(anchor: &Transform, size: Vec2) -> Rect {
    let half = size * 0.5;
    let matrix = anchor.to_matrix();
    let axis_x = matrix.transform_vector3(Vec3::X).truncate();
    let axis_y = matrix.transform_vector3(Vec3::Y).truncate();
    let world_half = (axis_x * half.x).abs() + (axis_y * half.y).abs();
    Rect::from_center_size(anchor.translation.truncate(), world_half * 2.0)
}

/// 有效裁剪矩形 = 面板自身矩形 ∩ `clip`（`None` 时就是面板自身）。
///
/// 这是一个纯函数：完全在视口外时返回**空矩形**（`is_empty()` 为真，
/// `min == max`，宽/高都是 0），没有 panic 路径。材质要的正是这个矩形。
pub fn effective_clip(panel: Rect, clip: Option<Rect>) -> Rect {
    match clip {
        Some(clip) => panel.intersect(clip),
        None => panel,
    }
}

// ---------------------------------------------------------------------------
// 物理像素 -> 逻辑像素 -> 世界米
// ---------------------------------------------------------------------------
//
// `ComputedNode` 的尺寸、边框、圆角，以及 `UiGlobalTransform` 的位移都是
// **物理像素**（`bevy_ui-0.20.1/src/ui_node.rs` 的文档与
// `computed.inverse_scale_factor()` 的用法都说明了这一点）。`WorldPanel`
// 的 `units_to_world` 是按**逻辑像素**标定的（布局写进 `Node` 的 `Val::Px`
// 都是逻辑单位），所以镜像前必须乘节点的 `inverse_scale_factor`。
//
// 1× 缩放下 `inverse_scale_factor == 1.0`，IEEE 乘法乘 1.0 是精确的，因此
// 这些函数退化成旧写法，现有截图逐字节不变。

/// 物理像素尺寸 → 逻辑像素（`ComputedNode.size * inverse_scale_factor`）。
#[inline]
pub fn physical_to_logical(physical: Vec2, inverse_scale_factor: f32) -> Vec2 {
    physical * inverse_scale_factor
}

/// 物理像素尺寸 → 世界米：先回逻辑像素，再乘面板的 `units_to_world`。
#[inline]
pub fn physical_size_to_world(
    physical: Vec2,
    inverse_scale_factor: f32,
    units_to_world: f32,
) -> Vec2 {
    physical * (inverse_scale_factor * units_to_world)
}

/// 物理像素长度（边框宽、圆角半径）→ 世界米。
#[inline]
pub fn physical_len_to_world(
    physical: f32,
    inverse_scale_factor: f32,
    units_to_world: f32,
) -> f32 {
    physical * (inverse_scale_factor * units_to_world)
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
    /// 上次写进材质的裁剪 uniform；`clip` 或面板位置变化时据此重写。
    pub cached_clip: Option<ClipUniform>,
}

/// 材质裁剪的 uniform 三元组：世界→裁剪空间矩阵前两行 + 半尺寸（世界米）。
///
/// 见 [`panel_clip_uniform`]；`None` 表示这一帧还没算过。
pub type ClipUniform = (Vec4, Vec4, Vec2);

/// One mirrored [`Outline`] ring (feathers' focus indicator uses `Outline`).
///
/// An outline is drawn **outside** the node: inner edge at `size + 2 * offset`,
/// outer edge at `size + 2 * (offset + width)`. We mirror it as a separate
/// quad the size of the outer box whose SDF material paints only its border
/// ring (no image, same `UNodeMaterial3d` as the node fills).
#[derive(Component)]
pub struct PanelOutlineNode {
    pub mesh: Handle<Mesh>,
    pub material: Handle<UNodeMaterial3d>,
    pub cached_size: Vec2,
    pub cached_color: Vec4,
    pub cached_radius: Vec4,
    pub cached_width: f32,
    /// 上次写进材质的裁剪 uniform（和 [`PanelNode::cached_clip`] 同口径）。
    pub cached_clip: Option<ClipUniform>,
}

/// [`outline_to_world`] 的结果（世界米），纯数据方便测试。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorOutline {
    /// 外框的世界尺寸（节点 + 2*(offset+width)）。
    pub size: Vec2,
    /// 描边宽度（世界米）。
    pub width: f32,
    /// 外框四角圆角（TR、BR、TL、BL，与 `PanelNode` 的 `cached_radius` 同序）。
    pub radius: Vec4,
    /// 描边颜色（线性 RGBA）。
    pub color: Vec4,
}

/// 节点四角圆角（物理像素）→ 世界米；顺序与材质 uniform 一致。
fn radius_to_world(
    radius: &ResolvedBorderRadius,
    inverse_scale_factor: f32,
    units_to_world: f32,
) -> Vec4 {
    let len = |corner: Vec2| {
        physical_len_to_world(
            corner.x.max(corner.y),
            inverse_scale_factor,
            units_to_world,
        )
    };
    Vec4::new(
        len(radius.top_right),
        len(radius.bottom_right),
        len(radius.top_left),
        len(radius.bottom_left),
    )
}

/// Resolve an [`Outline`] into the data its 3D mirror needs.
///
/// Width/offset come from the **laid-out** [`ComputedNode`] (the font-relative
/// and percentage `Val`s are already resolved there, in physical pixels), the
/// color comes from the [`Outline`] component. A zero-width or transparent
/// outline mirrors nothing (`update_row_focus_rings` parks the ring at
/// `Color::NONE` instead of removing it).
pub fn outline_to_world(
    node: &ComputedNode,
    outline: &Outline,
    inverse_scale_factor: f32,
    units_to_world: f32,
) -> Option<MirrorOutline> {
    let width_px = node.outline_width();
    let offset_px = node.outline_offset();
    if width_px <= 0.0 || outline.color.alpha() <= 0.0 {
        return None;
    }
    let outer_px = node.size + Vec2::splat(2.0 * (offset_px + width_px));
    Some(MirrorOutline {
        size: physical_size_to_world(outer_px, inverse_scale_factor, units_to_world),
        width: physical_len_to_world(width_px, inverse_scale_factor, units_to_world),
        radius: radius_to_world(&node.outline_radius(), inverse_scale_factor, units_to_world),
        color: LinearRgba::from(outline.color).to_vec4(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn sync_mirror(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
    panels: Query<(Entity, &WorldPanel, Option<&WorldLayer>), With<MirrorRoot>>,
    mut existing: Query<&mut PanelNode>,
    mut outlines: Query<(Entity, &mut PanelOutlineNode, &mut PanelSort)>,
    all_nodes: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&BackgroundColor>,
            Option<&BorderColor>,
            Option<&Outline>,
        ),
        With<Node>,
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
) {
    // `inverse_scale_factor` comes from the panel root: every node of a panel is laid
    // out against the same target and therefore shares it. Mirror geometry is built in
    // *logical* pixels, so all `ComputedNode`/`UiGlobalTransform` values (which bevy
    // reports in physical pixels) go through these helpers.
    let panel_map: HashMap<Entity, (f32, Transform, u16, f32, Option<Rect>)> = panels
        .iter()
        .map(|(e, p, layer)| {
            let inverse_scale_factor = all_nodes
                .get(e)
                .map_or(1.0, |(_, cn, _, _, _, _)| cn.inverse_scale_factor);
            (
                e,
                (
                    p.units_to_world,
                    p.anchor,
                    layer.map_or(0, |l| l.0),
                    inverse_scale_factor,
                    p.clip,
                ),
            )
        })
        .collect();
    if panel_map.is_empty() {
        return;
    }

    // Per-panel clip: UI subtree content is clipped to the panel's own box, which is what a
    // transcript needs once it scrolls. A panel may additionally carry a strip viewport
    // (`WorldPanel::clip`, world-axis-aligned); the effective clip is the intersection.
    let mut panel_clip: HashMap<Entity, ClipUniform> = HashMap::new();
    for root in panel_map.keys() {
        let Ok((_, cn, _, _, _, _)) = all_nodes.get(*root) else {
            continue;
        };
        let (units, anchor, _, inverse_scale_factor, clip) = panel_map
            .get(root)
            .copied()
            .unwrap_or((0.001, Transform::default(), 0, 1.0, None));
        // bounds are in the *clip space* the shader works in: metres, not UI pixels
        let half = physical_size_to_world(cn.size * 0.5, inverse_scale_factor, units);
        panel_clip.insert(*root, panel_clip_uniform(&anchor, half, clip));
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
        let layer = panel_map.get(root).map_or(0, |(_, _, layer, _, _)| *layer);
        let layer_bias = layer as f32 * LAYER_BIAS;
        panel_max_bias.insert(*root, layer_bias + order.saturating_sub(1) as f32 * SORT_STEP);
    }
    for (root, max_bias) in panel_max_bias {
        let layer = panel_map.get(&root).map_or(0, |(_, _, layer, _, _)| *layer);
        commands.entity(root).insert(PanelSortRange { max_bias, layer });
    }

    // Outline 是节点**外**的另一圈几何（feathers 的焦点环）。每个节点最多一个
    // `Outline` 组件，先收成 owner -> 镜像实体；处理节点时认领，处理完还剩下
    // 的就是这一帧已经不需要的环（颜色 `NONE` / 组件被移除 / 面板不在）。
    let mut outline_of: HashMap<Entity, Entity> = HashMap::new();
    for (outline_entity, _, _) in outlines.iter() {
        if let Ok(child_of) = parents.get(outline_entity) {
            outline_of.insert(child_of.parent(), outline_entity);
        }
    }

    for (node, (panel_root, depth, order)) in in_panel.iter() {
        let Some((units_to_world, anchor, layer, inverse_scale_factor, _)) =
            panel_map.get(panel_root).copied()
        else {
            continue;
        };
        let clip = panel_clip.get(panel_root).copied();
        let layer_bias = layer as f32 * LAYER_BIAS;
        let Ok((entity, cn, ugt, bg, border_color, outline)) = all_nodes.get(*node) else {
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

        // `ComputedNode` 是物理像素；乘 inverse_scale_factor 回到逻辑像素，
        // 再乘 units_to_world 得世界米（见文件上方的换算说明）。
        let size_world = physical_size_to_world(cn.size, inverse_scale_factor, units_to_world);

        let parent_centre_offset = match parents.get(entity) {
            Ok(child_of) => match all_nodes.get(child_of.parent()) {
                Ok((_, _, pugt, _, _, _)) => {
                    ugt.affine().translation - pugt.affine().translation
                }
                Err(_) => Vec2::ZERO,
            },
            Err(_) => Vec2::ZERO,
        };

        // `UiGlobalTransform` 的位移同样是物理像素。
        let parent_centre_offset = physical_size_to_world(
            parent_centre_offset,
            inverse_scale_factor,
            units_to_world,
        );
        let local_pos = Vec3::new(
            parent_centre_offset.x,
            -parent_centre_offset.y,
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
        // Outline 镜像：没有底色的节点（例如键盘选中的转写行只有 Outline）
        // 也要画，所以这段必须在 `!has_fill` 提前返回之前。
        if let Some(wanted) =
            outline.and_then(|outline| outline_to_world(cn, outline, inverse_scale_factor, units_to_world))
        {
            let bias = layer_bias + *order as f32 * SORT_STEP + SORT_STEP * 0.5;
            match outline_of.remove(&entity) {
                Some(outline_entity) => {
                    if let Ok((_, mut node3d, mut sort)) = outlines.get_mut(outline_entity) {
                        sort.center = anchor.translation;
                        sort.bias = bias;
                        if node3d.cached_size != wanted.size {
                            let mesh = meshes.add(Rectangle::new(wanted.size.x, wanted.size.y));
                            node3d.mesh = mesh.clone();
                            node3d.cached_size = wanted.size;
                            commands.entity(outline_entity).insert(Mesh3d(mesh));
                        }
                        if node3d.cached_color != wanted.color
                            || node3d.cached_radius != wanted.radius
                            || !(node3d.cached_width - wanted.width).abs().lt(&1e-9)
                            || node3d.cached_clip != clip
                        {
                            if let Some(mut mat) = materials.get_mut(&node3d.material) {
                                apply_panel_clip(&mut mat, clip.as_ref());
                                mat.color = LinearRgba::NONE.to_vec4();
                                mat.border_color = wanted.color;
                                mat.radius = wanted.radius;
                                mat.size = wanted.size;
                                mat.border_width = wanted.width;
                                mat.softness = 1.0e-5;
                                node3d.cached_color = wanted.color;
                                node3d.cached_radius = wanted.radius;
                                node3d.cached_width = wanted.width;
                                node3d.cached_clip = clip;
                            }
                        }
                    }
                }
                None => {
                    let (clip_row0, clip_row1, clip_half) =
                        clip.unwrap_or((Vec4::ZERO, Vec4::ZERO, Vec2::ZERO));
                    let material = UNodeMaterial3d {
                        clip_inv_x: [clip_row0, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO],
                        clip_inv_y: [clip_row1, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO],
                        clip_bounds: [
                            Vec4::new(clip_half.x, clip_half.y, 1.0, 0.0),
                            Vec4::ZERO,
                            Vec4::ZERO,
                            Vec4::ZERO,
                        ],
                        // 只画外框的 border 环，中间透明。
                        color: LinearRgba::NONE.to_vec4(),
                        radius: wanted.radius,
                        border_color: wanted.color,
                        size: wanted.size,
                        border_width: wanted.width,
                        softness: 1.0e-5,
                        roughness: 0.55,
                        ..default()
                    };
                    let mesh = meshes.add(Rectangle::new(wanted.size.x, wanted.size.y));
                    let material = materials.add(material);
                    commands.spawn((
                        PanelOutlineNode {
                            mesh: mesh.clone(),
                            material: material.clone(),
                            cached_size: wanted.size,
                            cached_color: wanted.color,
                            cached_radius: wanted.radius,
                            cached_width: wanted.width,
                            cached_clip: clip,
                        },
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        // 挂在节点下：节点自己的 Transform 是相对 UI 父层的局部位姿，
                        // 零位移的环正好和节点同心。
                        Transform::default(),
                        ChildOf(entity),
                        PanelSort {
                            center: anchor.translation,
                            bias,
                        },
                    ));
                }
            }
        }

        if !has_fill {
            continue;
        }

        let base_color = bg
            .map(|c| LinearRgba::from(c.0))
            .unwrap_or(LinearRgba::NONE);
        let b_color = border_color
            .map(|c| LinearRgba::from(c.top))
            .unwrap_or(LinearRgba::NONE);
        let b_width = physical_len_to_world(border_px, inverse_scale_factor, units_to_world);

        let r = &cn.border_radius;
        let radius_vec = radius_to_world(r, inverse_scale_factor, units_to_world);

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
                || node3d.cached_clip != clip
            {
                if let Some(mut mat) = materials.get_mut(&node3d.material) {
                    apply_panel_clip(&mut mat, clip.as_ref());
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
                    node3d.cached_clip = clip;
                }
            }
        } else {
            let (clip_row0, clip_row1, clip_half) =
                clip.unwrap_or((Vec4::ZERO, Vec4::ZERO, Vec2::ZERO));
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
                    cached_clip: clip,
                },
                Mesh3d(mesh),
                MeshMaterial3d(material),
            ));
        }
    }

    // 这一帧没认领的环（焦点走了 / 颜色变 NONE / 节点不再要 Outline）：拆掉。
    for (_, outline_entity) in outline_of {
        commands.entity(outline_entity).despawn();
    }
}

/// 面板的材质裁剪 uniform（节点与 Outline 环共用）。
///
/// * `clip = None`：旧口径——裁剪空间 = 面板局部坐标（世界→局部矩阵前两行），
///   原点在面板中心，半尺寸 = 面板半宽高；与加 `clip` 字段前逐字节一致。
/// * `clip = Some(rect)`：裁剪空间 = 世界坐标平移到"有效裁剪矩形"的中心
///   （世界轴对齐），半尺寸 = `面板 ∩ rect` 的一半。完全在视口外时半尺寸
///   为 0（shader 会丢弃所有片元，不 panic），面板实体仍然存在。
/// * 视口把面板整个包住（scroll = 0、没有伸出列）时退回旧口径：材质 uniform
///   与旧实现逐字节相同，画面不差一像素。
fn panel_clip_uniform(anchor: &Transform, half: Vec2, clip: Option<Rect>) -> ClipUniform {
    let local = || {
        let m = anchor.to_matrix().inverse().to_cols_array_2d();
        (
            Vec4::new(m[0][0], m[1][0], m[2][0], m[3][0]),
            Vec4::new(m[0][1], m[1][1], m[2][1], m[3][1]),
            half,
        )
    };
    let Some(clip) = clip else {
        return local();
    };
    let panel = panel_world_rect(anchor, half * 2.0);
    // 视口完整包住面板：交集就是面板自身，用旧口径（逐字节不变）。
    if clip.contains(panel.min) && clip.contains(panel.max) {
        return local();
    }
    let effective = effective_clip(panel, Some(clip));
    let center = effective.center();
    // 空交集：半尺寸 0；shader 的 clip_bounds.xy 为 0，所有片元都被丢弃。
    let half = if effective.is_empty() {
        Vec2::ZERO
    } else {
        effective.half_size()
    };
    (
        Vec4::new(1.0, 0.0, 0.0, -center.x),
        Vec4::new(0.0, 1.0, 0.0, -center.y),
        half,
    )
}

/// 把面板的裁剪 uniform 写进材质（节点与 Outline 环共用）。
fn apply_panel_clip(mat: &mut UNodeMaterial3d, clip: Option<&ClipUniform>) {
    if let Some((row0, row1, half)) = clip {
        mat.clip_inv_x = [*row0, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO];
        mat.clip_inv_y = [*row1, Vec4::ZERO, Vec4::ZERO, Vec4::ZERO];
        mat.clip_bounds = [
            Vec4::new(half.x, half.y, 1.0, 0.0),
            Vec4::ZERO,
            Vec4::ZERO,
            Vec4::ZERO,
        ];
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// `WorldPanel::units_to_world` 按逻辑像素标定（与 `frame::UNITS_TO_WORLD` 同值）。
    const UNITS: f32 = 0.0018;

    /// 1× 缩放：`inverse_scale_factor == 1.0` 时换算必须逐字节等于旧写法。
    #[test]
    fn one_x_scale_is_the_old_physical_equals_logical_behaviour() {
        let physical = Vec2::new(1234.5, 678.25);
        assert_eq!(
            physical_to_logical(physical, 1.0),
            physical,
            "1× 下逻辑像素 = 物理像素"
        );
        assert_eq!(
            physical_size_to_world(physical, 1.0, UNITS),
            physical * UNITS,
            "1× 下世界尺寸就是旧的 size * units_to_world"
        );
        assert_eq!(
            physical_len_to_world(7.5, 1.0, UNITS),
            7.5 * UNITS,
            "1× 下边框/圆角长度也原样"
        );
    }

    /// 2× 屏：ComputedNode 报 2 倍物理像素，回到逻辑像素后世界尺寸应与 1× 相同。
    #[test]
    fn two_x_physical_pixels_map_to_the_same_world_size_as_one_x() {
        let logical = Vec2::new(1280.0, 800.0);
        let physical = logical * 2.0;

        assert_eq!(
            physical_to_logical(physical, 0.5),
            logical,
            "物理 2560px 在 2× 下是逻辑 1280px"
        );
        assert_eq!(
            physical_size_to_world(physical, 0.5, UNITS),
            logical * UNITS,
            "2× 屏的 2560px 面板与 1× 屏的 1280px 面板在世界里一样大"
        );
        assert_eq!(
            physical_len_to_world(4.0, 0.5, UNITS),
            2.0 * UNITS,
            "2× 下 4 物理像素的边框 = 2 逻辑像素"
        );
    }

    /// 1.5× 屏：inverse_scale_factor 是 1/1.5，换算回同一个逻辑尺寸。
    #[test]
    fn one_and_a_half_x_physical_pixels_map_back_to_logical() {
        let logical = Vec2::new(1280.0, 800.0);
        let physical = logical * 1.5;
        let inverse_scale_factor = 1.0 / 1.5;

        let back = physical_to_logical(physical, inverse_scale_factor);
        assert!(
            (back - logical).length() < 1e-3,
            "1.5× 物理 {physical:?} -> 逻辑 {back:?}（期望 {logical:?}）"
        );

        let world = physical_size_to_world(physical, inverse_scale_factor, UNITS);
        let expected = logical * UNITS;
        assert!(
            (world - expected).length() < 1e-4,
            "1.5× 世界尺寸 {world:?}（期望 {expected:?}）"
        );

        let border = physical_len_to_world(3.0, inverse_scale_factor, UNITS);
        assert!((border - 2.0 * UNITS).abs() < 1e-6, "3 物理像素 = 2 逻辑像素");
    }

    // ---- Outline 镜像（feathers 焦点环） ----

    fn focus_ring_color() -> Color {
        Color::srgb(0.2, 0.6, 1.0).with_alpha(0.5)
    }

    /// 纯换算：外框 = 节点 + 2*(offset+width)，描边宽度 = width，颜色来自 Outline。
    #[test]
    fn outline_geometry_is_the_outer_ring_feathers_draws() {
        let mut node = ComputedNode {
            size: Vec2::new(100.0, 20.0),
            ..default()
        };
        node.outline_width = 2.0;
        node.outline_offset = 2.0;
        node.border_radius = ResolvedBorderRadius {
            top_left: Vec2::splat(4.0),
            top_right: Vec2::splat(4.0),
            bottom_right: Vec2::splat(4.0),
            bottom_left: Vec2::splat(4.0),
        };
        let color = focus_ring_color();
        let outline = Outline::new(Val::Px(2.0), Val::Px(2.0), color);

        let ring = outline_to_world(&node, &outline, 1.0, UNITS).expect("有宽度的环要生成");
        assert_eq!(
            ring.size,
            Vec2::new(108.0, 28.0) * UNITS,
            "外框 = 节点 + 2*(2px offset + 2px width)"
        );
        assert!(
            (ring.width - 2.0 * UNITS).abs() < 1e-9,
            "描边宽度 = Outline width"
        );
        assert_eq!(
            ring.radius,
            Vec4::splat(8.0 * UNITS),
            "外框圆角 = 节点圆角 4px + offset+width 4px"
        );
        assert_eq!(ring.color, LinearRgba::from(color).to_vec4());

        // 透明 / 零宽不镜像（`update_row_focus_rings` 用 Color::NONE 关环）。
        let hidden = Outline::new(Val::Px(2.0), Val::Px(2.0), Color::NONE);
        assert!(outline_to_world(&node, &hidden, 1.0, UNITS).is_none());
        node.outline_width = 0.0;
        assert!(outline_to_world(&node, &outline, 1.0, UNITS).is_none());
    }

    /// 组件级：给一个**没有底色**的节点加 Outline，`sync_mirror` 要挂一个
    /// 独立的描边实体（带材质和世界尺寸）；移除 Outline 后它要被拆掉。
    #[test]
    fn sync_mirror_spawns_a_ring_even_when_the_node_has_no_fill() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<UNodeMaterial3d>::default());

        let panel = world
            .spawn((
                MirrorRoot,
                WorldPanel {
                    units_to_world: UNITS,
                    anchor: Transform::from_xyz(0.0, 0.0, -1.0),
                    clip: None,
                },
                WorldLayer(0),
                Node::default(),
                ComputedNode {
                    size: Vec2::new(200.0, 100.0),
                    ..default()
                },
                UiGlobalTransform::default(),
            ))
            .id();

        let mut cn = ComputedNode {
            size: Vec2::new(100.0, 20.0),
            ..default()
        };
        cn.outline_width = 2.0;
        cn.outline_offset = 2.0;
        let color = focus_ring_color();
        let node = world
            .spawn((
                Node::default(),
                cn,
                UiGlobalTransform::from_translation(Vec2::new(50.0, 30.0)),
                BackgroundColor(Color::NONE),
                BorderColor::default(),
                Outline::new(Val::Px(2.0), Val::Px(2.0), color),
                ChildOf(panel),
            ))
            .id();

        world.run_system_once(sync_mirror).unwrap();

        // 没底色：节点自己不生成填充 mesh，但环要生成。
        assert!(
            world.get::<PanelNode>(node).is_none(),
            "透明节点不该有填充 mesh"
        );
        let ring_entity = world
            .get::<Children>(node)
            .expect("节点下挂着环")
            .iter()
            .find(|child| world.get::<PanelOutlineNode>(*child).is_some())
            .expect("应该有一个 PanelOutlineNode");
        assert_eq!(
            world.get::<ChildOf>(ring_entity).map(ChildOf::parent),
            Some(node),
            "环挂在被描边的节点下"
        );

        let ring = world.get::<PanelOutlineNode>(ring_entity).expect("环数据");
        let expected_size = Vec2::new(108.0, 28.0) * UNITS;
        assert!(
            (ring.cached_size - expected_size).length() < 1e-9,
            "环的世界尺寸 {:?}（期望 {expected_size:?}）",
            ring.cached_size
        );
        assert_eq!(ring.cached_color, LinearRgba::from(color).to_vec4());
        {
            let materials = world.resource::<Assets<UNodeMaterial3d>>();
            let material = materials.get(&ring.material).expect("环材质");
            assert_eq!(
                material.color,
                LinearRgba::NONE.to_vec4(),
                "中间透明，只画 border 环"
            );
            assert_eq!(material.border_color, LinearRgba::from(color).to_vec4());
            assert!(
                (material.border_width - 2.0 * UNITS).abs() < 1e-9,
                "border 宽度 = Outline width"
            );
            assert!(
                (material.size - expected_size).length() < 1e-9,
                "材质外框尺寸"
            );
        }

        // 焦点走了：Outline 移除后，环实体也要消失。
        world.entity_mut(node).remove::<Outline>();
        world.run_system_once(sync_mirror).unwrap();
        assert!(
            world.get_entity(ring_entity).is_err(),
            "没有 Outline 的节点不该留着环"
        );
    }

    // ---- 条带视口裁剪（L2） ----

    /// 有效裁剪 = 面板 ∩ clip；`None` 就是面板自身；完全在外时是空矩形。
    #[test]
    fn effective_clip_is_the_panel_intersect_the_viewport() {
        let panel = Rect::new(-1.0, -0.5, 1.0, 0.5);
        assert_eq!(effective_clip(panel, None), panel, "None = 不额外裁剪");

        let viewport = Rect::new(0.0, -1.0, 2.0, 1.0);
        assert_eq!(
            effective_clip(panel, Some(viewport)),
            Rect::new(0.0, -0.5, 1.0, 0.5),
            "左半边被裁掉"
        );

        // 完全在视口外：空矩形（min == max），不会 panic。
        let outside = Rect::new(5.0, 5.0, 6.0, 6.0);
        let empty = effective_clip(panel, Some(outside));
        assert!(empty.is_empty(), "无交集要塌成空矩形：{empty:?}");
        assert_eq!(empty.size(), Vec2::ZERO);

        // 空交集也能算出材质 uniform：半尺寸 0 = shader 丢弃所有片元。
        let (row0, row1, half) =
            panel_clip_uniform(&Transform::default(), Vec2::new(1.0, 0.5), Some(outside));
        assert_eq!(half, Vec2::ZERO);
        assert_eq!(row0, Vec4::new(1.0, 0.0, 0.0, -1.0), "裁剪空间原点塌到面板角");
        assert_eq!(row1, Vec4::new(0.0, 1.0, 0.0, -0.5));
    }

    /// ECS 级：`sync_mirror` 把材质裁剪设成 `面板 ∩ WorldPanel::clip`；
    /// 面板整个滚出视口时仍然存在（不 despawn），材质宽/高为 0。
    #[test]
    fn sync_mirror_clips_the_material_to_the_strip_viewport() {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<UNodeMaterial3d>::default());

        let panel = world
            .spawn((
                MirrorRoot,
                WorldPanel {
                    units_to_world: UNITS,
                    anchor: Transform::default(),
                    // 面板世界矩形 x ∈ [-0.18, 0.18]；视口只留右半边。
                    clip: Some(Rect::new(0.0, -1.0, 1.0, 1.0)),
                },
                WorldLayer(0),
                Node::default(),
                ComputedNode {
                    size: Vec2::new(200.0, 100.0),
                    ..default()
                },
                UiGlobalTransform::default(),
                BackgroundColor(Color::WHITE),
            ))
            .id();

        world.run_system_once(sync_mirror).unwrap();
        let node = world.get::<PanelNode>(panel).expect("有底色就有填充 mesh");
        let material = world
            .resource::<Assets<UNodeMaterial3d>>()
            .get(&node.material)
            .expect("材质");
        assert!(
            (material.clip_bounds[0] - Vec4::new(0.09, 0.09, 1.0, 0.0)).length() < 1e-6,
            "可见半尺寸 = 交集的一半：{:?}",
            material.clip_bounds[0]
        );
        assert!(
            (material.clip_inv_x[0] - Vec4::new(1.0, 0.0, 0.0, -0.09)).length() < 1e-6,
            "裁剪空间 x 原点在 0.09（交集中心）：{:?}",
            material.clip_inv_x[0]
        );
        assert!(
            (material.clip_inv_y[0] - Vec4::new(0.0, 1.0, 0.0, -0.0)).length() < 1e-6,
            "y 方向视口完整包住面板：{:?}",
            material.clip_inv_y[0]
        );

        // 滚出视口：面板还在，材质裁剪塌成 0×0（不 panic）。
        world.get_mut::<WorldPanel>(panel).unwrap().clip =
            Some(Rect::new(5.0, 5.0, 6.0, 6.0));
        world.run_system_once(sync_mirror).unwrap();
        assert!(world.get_entity(panel).is_ok(), "滚出去不 despawn");
        let node = world.get::<PanelNode>(panel).expect("mesh 还在");
        let material = world
            .resource::<Assets<UNodeMaterial3d>>()
            .get(&node.material)
            .expect("材质");
        assert_eq!(
            material.clip_bounds[0],
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            "空交集：可见尺寸 0×0"
        );
    }
}
