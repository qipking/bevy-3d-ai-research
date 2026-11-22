//! Route B spike v2: render `bevy_feathers`/`bevy_ui` nodes as 3D world-space panels,
//! reusing `bevy_luma`'s SDF 3D material (`UNodeMaterial3d`) and its `unode_3d.wesl` shader.
//!
//! Hypothesis under test (see `../RESETUP_DOC.md`):
//!   `bevy_ui`'s layout output (`ComputedNode` + `UiGlobalTransform`) can be mapped onto
//!   3D world-space geometry + `bevy_luma`'s SDF 3D material, with the panel staying crisp
//!   under arbitrary scale and remaining pickable through the normal `bevy_picking` path.
//!
//! What v2 adds over the handover version:
//!   * Uses `UnivisRenderPlugin` (material + embedded shaders only) instead of the whole
//!     `LumaPlugin`: luma's layout/interaction plugins form a dependency cycle with
//!     `bevy_ui`'s `PostUpdate` sets and panic on startup (see `notes` below).
//!   * Keeps the source `bevy_ui` tree *off-screen* so the 2D overlay never covers the 3D mirror.
//!   * Individual panel anchors (`Transform`) instead of "always at the world origin",
//!     plus a per-depth Z bias so nested nodes win depth sorting / picking.
//!   * No per-frame mesh/material leak: handles are cached and only rebuilt when stale.
//!   * A deterministic camera / screenshot / picking tour (`Runner`), so the whole verification
//!     runs unattended and writes PNGs.
//!   * A synthetic `bevy_picking` pointer that hovers + clicks the 3D button through the real
//!     `bevy_picking` pipeline (RayMap -> mesh picking backend -> PointerOver/PointerClick ->
//!     `bevy_ui_widgets::Button` -> `Activate`).
//!
//! Deliberate scope limits (unchanged): no scroll containers, no nested clipping, no 3D text.
//!
//! ## Notes: why `UnivisRenderPlugin` and not `LumaPlugin`
//!
//! `bevy_luma::LumaPlugin` = style + engine (layout/render/text) + interaction plugins.
//! Its layout systems are ordered against `UiSystems`-adjacent sets. Together with `bevy_ui`
//! 0.20.1 the `PostUpdate` schedule ends up with a before/after cycle and Bevy panics with
//! `schedule has 14522 before/after cycle(s)` before the first frame. Route B only needs
//! luma's *material* (`UNodeMaterial3d`) and *shader* (`unode_3d.wesl`), both registered by
//! `UnivisRenderPlugin`, so the spike uses that instead.

use std::collections::HashMap;

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::pbr::MeshMaterial3d;
use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::picking::pointer::{
    Location, PointerAction, PointerButton, PointerId, PointerInput, PointerLocation,
};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::text::TextLayoutInfo;
use bevy::window::{PrimaryWindow, WindowRef};
use bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d;
use bevy_luma::engine::layout::render::UnivisRenderPlugin;
use bevy_ui::{BackgroundColor, BorderColor, BorderRadius, ComputedNode, Node, UiGlobalTransform};
use bevy_ui_widgets::{Activate, Button};

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Extra local Z per hierarchy level, in world units. Keeps children "in front" of their
/// parent without changing layout: 1 mm at `units_to_world = 0.002`.
const Z_BIAS_PER_LEVEL: f32 = 0.001;

/// Panel A: the main panel, mirroring the handover spike layout.
const PANEL_A_SIZE: Vec2 = Vec2::new(340.0, 200.0);
const PANEL_A_UNITS_TO_WORLD: f32 = 0.002;
const PANEL_PADDING: f32 = 14.0;
const PANEL_ROW_GAP: f32 = 8.0;
const BUTTON_SIZE: Vec2 = Vec2::new(120.0, 32.0);
const BLOCK_SIZE: Vec2 = Vec2::new(180.0, 28.0);

/// Frames granted to each tour step to settle (layout, screenshot capture, hover state).
const STEP_FRAMES: u32 = 18;

// ---------------------------------------------------------------------------
// Components / resources
// ---------------------------------------------------------------------------

/// Marks a `bevy_ui` subtree as being rendered into the 3D world as a physical panel.
#[derive(Component, Clone, Copy, Debug)]
pub struct WorldPanel {
    /// Logical-UI-unit to world-space scale. `0.002` means 1 logical px == 2 mm.
    pub units_to_world: f32,
    /// Where the panel lives in the 3D world (rotation/scale are honoured).
    pub anchor: Transform,
}

/// Identifies the panel root so the mirror can walk its subtree.
#[derive(Component)]
struct MirrorRoot;

/// Per-node bookkeeping for the 3D mirror of the UI tree (handles are cached, never leaked).
#[derive(Component)]
struct PanelNode {
    mesh: Handle<Mesh>,
    material: Handle<UNodeMaterial3d>,
    depth: u32,
    cached_size: Vec2,
    cached_color: Vec4,
    cached_border: Vec4,
    cached_radius: Vec4,
    cached_border_width: f32,
}

/// Counters for the spike report.
#[derive(Resource, Default)]
struct SpikeStats {
    panels: u32,
    mirrored_nodes: u32,
}

#[derive(Component)]
struct ShotCamera;

/// The synthetic picking pointer (touch 7), so the tour can hover/click without a real mouse.
#[derive(Resource)]
struct ProbePointer(Entity);

#[derive(Resource, Default)]
struct Runner {
    frame: u32,
    shot_dir: String,
    shots_taken: Vec<String>,
    reported: bool,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let shot_dir = std::env::var("SPIKE_SHOT_DIR").unwrap_or_else(|_| "shots".to_string());
    std::fs::create_dir_all(&shot_dir).ok();

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Route B spike v2: feathers UI mirrored into 3D world space".into(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(bevy_feathers::FeathersPlugins)
        // Only luma's render plugin: material + embedded shaders. See module docs.
        .add_plugins(UnivisRenderPlugin)
        // Mesh ray casting -> PointerHits. This is what makes the 3D mirror pickable with
        // zero custom picking code; the rectangles are ordinary `Mesh3d`s.
        .add_plugins(MeshPickingPlugin)
        .init_resource::<SpikeStats>()
        .init_resource::<GlyphMaterials>()
        .insert_resource(Runner {
            shot_dir,
            ..default()
        })
        .add_systems(Startup, (setup, spawn_probe_pointer))
        .add_systems(PostUpdate, (sync_mirror, sync_text3d, report_once).chain())
        .add_systems(Update, run_tour)
        .add_observer(highlight_hover)
        .add_observer(highlight_out)
        .add_observer(on_click)
        .add_observer(on_activate)
        .run();
}

// ---------------------------------------------------------------------------
// Scene setup
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
) {
    let isolate = std::env::var("SPIKE_ISOLATE").is_ok();
    let panel_color = if isolate {
        Color::srgb(0.85, 0.05, 0.65)
    } else {
        Color::srgb(0.07, 0.10, 0.16)
    };
    // ---- 3D camera ------------------------------------------------------
    commands.spawn((
        ShotCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 45_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.22, 1.05).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 2500.0,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // ---- reference cube --------------------------------------------------
    // Sits *behind* panel A (z = -0.12) but pokes out to the right of it, so:
    //   - it proves the panel participates in normal 3D depth (occlusion),
    //   - it gives the eye a non-UI reference for perspective at every zoom level.
    if !isolate {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(0.14, 0.14, 0.14))),
        MeshMaterial3d(materials.add(UNodeMaterial3d {
            color: Vec4::new(0.95, 0.45, 0.12, 1.0),
            roughness: 0.6,
            pbr_enabled: 1,
            ..default()
        })),
            Transform::from_xyz(0.38, -0.10, -0.12),
        ));
    }

    // ---- panel A: the main feathers/bevy_ui tree --------------------------
    commands
        .spawn((
            // Placed far off-screen: `bevy_ui` still lays it out (that's all route B needs),
            // but the 2D `bevy_ui` render pipeline draws nothing over the 3D mirror.
            Node {
                position_type: PositionType::Absolute,
                left: px(-20000.0),
                top: px(0.0),
                width: px(PANEL_A_SIZE.x),
                height: px(PANEL_A_SIZE.y),
                padding: UiRect::all(px(PANEL_PADDING)),
                border: UiRect::all(px(2.0)),
                border_radius: BorderRadius::all(px(10.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(PANEL_ROW_GAP),
                align_items: AlignItems::FlexStart,
                ..default()
            },
            BackgroundColor(panel_color),
            BorderColor::all(Color::srgb(0.25, 0.62, 0.95)),
            WorldPanel {
                units_to_world: PANEL_A_UNITS_TO_WORLD,
                anchor: Transform::default(),
            },
            MirrorRoot,
        ))
        .with_children(|children| {
            // Title text: `bevy_ui` lays it out and reserves its height, but the mirror skips
            // text nodes (3D text is out of scope), so it shows up as a layout gap.
            children.spawn((
                Text::new("Feathers in 3D"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.97, 1.0)),
            ));
            // A `bevy_ui_widgets::Button` is the substrate `bevy_feathers`' `FeathersButton`
            // builds on; using it directly avoids the scene-spawn API while still exercising
            // the real pointer-event -> `Activate` widget path.
            children.spawn((
                Node {
                    width: px(BUTTON_SIZE.x),
                    height: px(BUTTON_SIZE.y),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(px(4.0)),
                    ..default()
                },
                Button,
                BackgroundColor(Color::srgb(0.15, 0.30, 0.50)),
            ));
            children.spawn((
                Node {
                    width: px(BLOCK_SIZE.x),
                    height: px(BLOCK_SIZE.y),
                    border_radius: BorderRadius::all(px(6.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.16, 0.22, 0.32)),
            ));
        });

    // ---- panel B: second, smaller, rotated panel -------------------------
    // Proves the mapping works for arbitrary anchors/rotations and for more than one panel.
    if !isolate {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(-30000.0),
                top: px(0.0),
                width: px(240.0),
                height: px(140.0),
                padding: UiRect::all(px(12.0)),
                border_radius: BorderRadius::all(px(14.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(8.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.10, 0.05, 0.16, 1.0)),
            BorderColor::all(Color::srgb(0.85, 0.45, 0.95)),
            WorldPanel {
                units_to_world: 0.0016,
                anchor: Transform::from_xyz(-0.56, 0.10, -0.22)
                    .with_rotation(Quat::from_rotation_y(35_f32.to_radians())),
            },
            MirrorRoot,
        ))
        .with_children(|children| {
            children.spawn((
                Node {
                    width: px(200.0),
                    height: px(22.0),
                    border_radius: BorderRadius::all(px(4.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.55, 0.25, 0.65)),
            ));
            children.spawn((
                Node {
                    width: px(60.0),
                    height: px(60.0),
                    border: UiRect::all(px(4.0)),
                    border_radius: BorderRadius::all(px(30.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.20, 0.10, 0.25)),
                BorderColor::all(Color::srgb(0.95, 0.85, 0.35)),
            ));
        });
    }
}

/// Spawns the synthetic pointer used by the tour to hover/click the 3D button.
fn spawn_probe_pointer(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    let target = NormalizedRenderTarget::Window(
        WindowRef::Entity(*window)
            .normalize(Some(*window))
            .expect("primary window normalizes"),
    );
    let pointer = commands
        .spawn((
            PointerId::Touch(7),
            PointerLocation::new(Location {
                target,
                position: Vec2::new(-1000.0, -1000.0),
            }),
        ))
        .id();
    commands.insert_resource(ProbePointer(pointer));
}

// ---------------------------------------------------------------------------
// The mirror: bevy_ui nodes -> 3D meshes
// ---------------------------------------------------------------------------

/// Creates/updates the 3D mesh + material for every UI node inside a `WorldPanel`.
///
/// Coordinate mapping (the crux of route B):
///   `bevy_ui` lays out in physical pixels with the viewport centre as origin.
///   `UiGlobalTransform` is an `Affine2` whose translation is the node's centre
///   relative to its parent's centre. For 3D we need, per node:
///     - local size   = `ComputedNode.size * units_to_world`
///     - local offset = child centre relative to parent centre, in the same units
///   The panel root itself takes the panel `anchor`; descendants keep parent-relative
///   transforms, so Bevy's normal transform propagation does the 3D math.
#[allow(clippy::too_many_arguments)]
fn sync_mirror(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
    mut stats: ResMut<SpikeStats>,
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
    stats.panels = panel_map.len() as u32;

    // Collect which entities belong to a panel subtree, and at what depth.
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

    let mut mirrored = 0u32;

    for (node, (panel_root, depth)) in in_panel.iter() {
        let Some((units_to_world, anchor)) = panel_map.get(panel_root).copied() else {
            continue;
        };

        let Ok((entity, cn, ugt, bg, border_color)) = all_nodes.get(*node) else {
            continue;
        };

        // Text nodes (and anything else without a fill) are not mirrored: 3D text is out of
        // scope for the spike. Their layout space is still reserved, which is itself useful
        // evidence that the layout engine drives the 3D result.
        let border_px = cn
            .border
            .min_inset
            .x
            .max(cn.border.min_inset.y)
            .max(cn.border.max_inset.x)
            .max(cn.border.max_inset.y);
        let has_fill = bg.is_some_and(|c| c.0.alpha() > 0.0) || border_px > 0.0;
        if !has_fill {
            continue;
        }

        // --- geometry ---------------------------------------------------
        let size_world = cn.size * units_to_world;

        // The node's centre relative to its parent's centre, in pixels.
        let parent_centre_offset = match parents.get(entity) {
            Ok(child_of) => {
                let p = child_of.parent();
                match all_nodes.get(p) {
                    Ok((_, _, pugt, _, _)) => {
                        let child_pos = ugt.affine().translation;
                        let parent_pos = pugt.affine().translation;
                        child_pos - parent_pos
                    }
                    Err(_) => Vec2::ZERO,
                }
            }
            Err(_) => Vec2::ZERO,
        };

        // Bevy UI Y grows downward; 3D world Y grows upward. Z bias keeps children in front.
        let local_pos = Vec3::new(
            parent_centre_offset.x * units_to_world,
            -parent_centre_offset.y * units_to_world,
            *depth as f32 * Z_BIAS_PER_LEVEL,
        );

        // --- material params --------------------------------------------
        let base_color = bg
            .map(|c| LinearRgba::from(c.0))
            .unwrap_or(LinearRgba::NONE);
        let b_color = border_color
            .map(|c| LinearRgba::from(c.top))
            .unwrap_or(LinearRgba::NONE);
        let b_width = border_px * units_to_world;

        // luma's `UNodeMaterial3d.radius` is (top_right, bottom_right, top_left, bottom_left).
        let r = &cn.border_radius;
        let radius_vec = Vec4::new(
            r.top_right.x.max(r.top_right.y) * units_to_world,
            r.bottom_right.x.max(r.bottom_right.y) * units_to_world,
            r.top_left.x.max(r.top_left.y) * units_to_world,
            r.bottom_left.x.max(r.bottom_left.y) * units_to_world,
        );

        let color4 = base_color.to_vec4();
        let border4 = b_color.to_vec4();

        // --- attach / update ---------------------------------------------
        if let Ok(mut node3d) = existing.get_mut(entity) {
            // Mesh: only rebuild when the layout size actually changed.
            if node3d.cached_size != size_world {
                let mesh = meshes.add(Rectangle::new(size_world.x, size_world.y));
                node3d.mesh = mesh.clone();
                node3d.cached_size = size_world;
                commands.entity(entity).insert(Mesh3d(mesh));
            }
            node3d.depth = *depth;

            // Material: only touch the asset when a style input changed (keeps hover stable).
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

            let desired = if *depth == 0 {
                anchor
            } else {
                Transform::from_translation(local_pos)
            };
            commands.entity(entity).insert(desired);
        } else {
            let material = UNodeMaterial3d {
                color: color4,
                radius: radius_vec,
                border_color: border4,
                size: size_world,
                border_width: b_width,
                // 3D uses screen-space derivatives for AA (`unode_3d.wesl`), so the softness
                // floor can be ~0 -> crisp at any scale.
                softness: 1.0e-5,
                metallic: 0.0,
                roughness: 0.55,
                use_texture: 0,
                shape_mode: 0,
                pbr_enabled: 0,
                ..default()
            };
            let mesh = meshes.add(Rectangle::new(size_world.x, size_world.y));
            let material = materials.add(material);
            let transform = if *depth == 0 {
                anchor
            } else {
                Transform::from_translation(local_pos)
            };
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
                transform,
            ));
        }
        mirrored += 1;
    }

    stats.mirrored_nodes = mirrored;
}

// ---------------------------------------------------------------------------
// 3D text: bevy_text's laid-out glyphs -> one textured quad mesh per text node
// ---------------------------------------------------------------------------
///
/// `bevy_text` already shapes, wraps and positions every glyph (`TextLayoutInfo`),
/// and the glyph atlas is a plain `Rgba8UnormSrgb` image. So 3D text is "just"
/// a quad per glyph: position from `TextLayoutInfo`, uv from `GlyphAtlasInfo.rect`,
/// color from `TextColor` as a vertex color, and one `StandardMaterial` per atlas.
/// No custom shader, no re-shaping, and it follows the same UI layout as 2D.

/// Cache of one `StandardMaterial` per font-atlas texture.
#[derive(Resource, Default)]
struct GlyphMaterials(HashMap<AssetId<Image>, Handle<StandardMaterial>>);

/// Bookkeeping for a text node's glyph mesh.
#[derive(Component)]
struct Text3d {
    mesh: Handle<Mesh>,
    glyphs: usize,
}

fn sync_text3d(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    mut glyph_materials: ResMut<GlyphMaterials>,
    panels: Query<(Entity, &WorldPanel), With<MirrorRoot>>,
    texts: Query<
        (Entity, &TextLayoutInfo, &ComputedNode, &UiGlobalTransform, Option<&TextColor>),
        (With<Text>, Changed<TextLayoutInfo>),
    >,
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

    for (entity, info, cn, ugt, text_color) in texts.iter() {
        if info.glyphs.is_empty() {
            continue;
        }

        // Which panel is this text in, and how deep?
        let mut current = entity;
        let mut depth = 0u32;
        let mut panel: Option<(Entity, f32)> = None;
        loop {
            if let Some(u) = panel_map.get(&current) {
                panel = Some((current, *u));
                break;
            }
            let Ok(child_of) = parents.get(current) else {
                break;
            };
            current = child_of.parent();
            depth += 1;
            if depth > 64 {
                break;
            }
        }
        let Some((_, units_to_world)) = panel else {
            continue;
        };

        // Same parent-relative placement the mirror uses.
        let parent_centre_offset = match parents.get(entity) {
            Ok(child_of) => match all_nodes.get(child_of.parent()) {
                Ok((_, pugt)) => ugt.affine().translation - pugt.affine().translation,
                Err(_) => Vec2::ZERO,
            },
            Err(_) => Vec2::ZERO,
        };

        let content_min = cn.content_box().min;
        let color = text_color
            .map(|c| LinearRgba::from(c.0).to_f32_array())
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);

        // Group the glyphs by atlas texture: one mesh+material per atlas.
        let mut by_atlas: HashMap<AssetId<Image>, Vec<(&bevy::math::Vec2, &bevy::math::Rect)>> =
            HashMap::new();
        for glyph in info.glyphs.iter() {
            by_atlas
                .entry(glyph.atlas_info.texture)
                .or_default()
                .push((&glyph.position, &glyph.atlas_info.rect));
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

            for (position, rect) in glyphs.iter() {
                let centre = content_min + **position; // px, relative to the node centre
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
                colors.extend_from_slice(&[color; 4]);

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
                depth as f32 * Z_BIAS_PER_LEVEL + 0.0004,
            ));

            if let Ok(old) = existing.get(entity) {
                commands.entity(entity).insert((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    transform,
                    Text3d { mesh: mesh.clone(), glyphs: glyphs.len() },
                ));
                let _ = &old.mesh;
            } else {
                commands.entity(entity).insert((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    transform,
                    Text3d { mesh: mesh.clone(), glyphs: glyphs.len() },
                ));
            }
            info!(
                "[spike] text3d entity={entity:?} glyphs={} atlas={:?}",
                glyphs.len(),
                atlas_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Interaction: hover highlight + click logging
// ---------------------------------------------------------------------------

fn highlight_hover(
    over: On<bevy::picking::events::PointerOver>,
    nodes: Query<&PanelNode>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
) {
    // Only the synthetic probe pointer highlights, so unattended tours stay deterministic
    // (a real OS mouse hovering the panel would otherwise brighten whole screenshots).
    if over.pointer.id != PointerId::Touch(7) {
        return;
    }
    let entity = over.entity;
    let Ok(node) = nodes.get(entity) else {
        return;
    };
    if let Some(mut mat) = materials.get_mut(&node.material) {
        let c = node.cached_color;
        mat.color = Vec4::new(
            (c.x + 0.25).min(1.0),
            (c.y + 0.25).min(1.0),
            (c.z + 0.25).min(1.0),
            c.w,
        );
    }
    info!("[spike] PointerOver  entity={entity:?}");
}

fn highlight_out(
    out: On<bevy::picking::events::PointerOut>,
    nodes: Query<&PanelNode>,
    mut materials: ResMut<Assets<UNodeMaterial3d>>,
) {
    let entity = out.entity;
    let Ok(node) = nodes.get(entity) else {
        return;
    };
    if let Some(mut mat) = materials.get_mut(&node.material) {
        mat.color = node.cached_color;
    }
    info!("[spike] PointerOut   entity={entity:?}");
}

fn on_click(click: On<bevy::picking::events::PointerClick>) {
    info!(
        "[spike] PointerClick entity={:?} pointer={:?} button={:?} depth={:.4}",
        click.entity, click.pointer.id, click.button, click.hit.depth
    );
}

fn on_activate(activate: On<Activate>) {
    info!("[spike] ACTIVATE entity={:?}  <-- widget fired", activate.entity);
}

// ---------------------------------------------------------------------------
// Report (U4 assertions, printed once)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn report_once(
    mut runner: ResMut<Runner>,
    stats: Res<SpikeStats>,
    nodes: Query<(Entity, &PanelNode, &GlobalTransform, &ComputedNode)>,
    panels: Query<(Entity, &WorldPanel, &ComputedNode), With<MirrorRoot>>,
    children_q: Query<&Children>,
    all_nodes: Query<(&ComputedNode, Option<&GlobalTransform>), With<Node>>,
    buttons: Query<Entity, With<Button>>,
) {
    if runner.reported || runner.frame < 5 {
        return;
    }
    runner.reported = true;

    let count = nodes.iter().count();
    info!(
        "[spike] panels={} mirrored_nodes={} live_mirrors={}",
        stats.panels, stats.mirrored_nodes, count
    );

    // ---- U4 quantitative check -----------------------------------------
    // Expected 3D positions are recomputed from *layout sizes only* (`ComputedNode`) plus the
    // known CSS constants (border 2, padding 14, row gap 8, column flow). The arithmetic does
    // not touch `UiGlobalTransform`, so agreement with the mirror's output is a genuine check
    // of the layout -> 3D mapping (not a tautology).
    let mut checked: Vec<(Entity, Vec3, Vec3)> = Vec::new();
    for (root, panel, root_cn) in panels.iter() {
        if panel.anchor.translation != Vec3::ZERO {
            continue; // only the axis-aligned panel at the origin has hand-checkable numbers
        }
        let u = panel.units_to_world;
        let border = root_cn.border.min_inset.x;
        let padding = root_cn.padding.min_inset.x;
        info!(
            "[spike] U4 root_cn: size=({:.1},{:.1}) border_min=({:.1},{:.1}) border_max=({:.1},{:.1}) \
             padding_min=({:.1},{:.1}) padding_max=({:.1},{:.1})",
            root_cn.size.x, root_cn.size.y,
            root_cn.border.min_inset.x, root_cn.border.min_inset.y,
            root_cn.border.max_inset.x, root_cn.border.max_inset.y,
            root_cn.padding.min_inset.x, root_cn.padding.min_inset.y,
            root_cn.padding.max_inset.x, root_cn.padding.max_inset.y,
        );
        let gap = 8.0; // [SPIKE] must match the `row_gap` below
        let mut cursor_y = -root_cn.size.y * 0.5 + border + padding;
        let content_left = -root_cn.size.x * 0.5 + border + padding;

        let Ok(kids) = children_q.get(root) else {
            continue;
        };
        // Every flex child consumes a slot (even ones we do not mirror, like text),
        // because `row_gap` is applied between all items regardless of their height.
        for (idx, kid) in kids.iter().enumerate() {
            if idx > 0 {
                cursor_y += gap;
            }
            let Ok((cn, _gt)) = all_nodes.get(kid) else {
                continue;
            };
            let expected_local = Vec3::new(
                (content_left + cn.size.x * 0.5) * u,
                -(cursor_y + cn.size.y * 0.5) * u,
                Z_BIAS_PER_LEVEL,
            );
            cursor_y += cn.size.y;
            if let Ok((_, _, gt, _)) = nodes.get(kid) {
                checked.push((kid, gt.translation(), expected_local));
            } else {
                info!(
                    "[spike] U4 skipped (not mirrored, e.g. text): {kid:?} size_px=({:.1},{:.1})",
                    cn.size.x, cn.size.y
                );
            }
        }
    }

    for (entity, actual, expected) in checked.iter() {
        let delta = *actual - *expected;
        let is_button = buttons.contains(*entity);
        info!(
            "[spike] U4 {} {entity:?}: actual=({:.4},{:.4},{:.4}) expected=({:.4},{:.4},{:.4}) \
             delta=({:.5},{:.5},{:.5})",
            if is_button { "button" } else { "node  " },
            actual.x, actual.y, actual.z,
            expected.x, expected.y, expected.z,
            delta.x, delta.y, delta.z
        );
    }

    for (root, panel, _) in panels.iter() {
        info!(
            "[spike] panel root {root:?}: units_to_world={} anchor_pos={:?}",
            panel.units_to_world, panel.anchor.translation
        );
    }

    // Every mirrored node with its placement, for the record.
    for (entity, node, gt, cn) in nodes.iter() {
        let t = gt.translation();
        info!(
            "[spike] node {entity:?} depth={} pos=({:.4},{:.4},{:.4}) size_px=({:.1},{:.1})",
            node.depth, t.x, t.y, t.z, cn.size.x, cn.size.y
        );
    }
}

// ---------------------------------------------------------------------------
// Deterministic tour: camera moves, screenshots, synthetic hover + click
// ---------------------------------------------------------------------------

fn set_camera(
    camera: &mut Transform,
    pos: Vec3,
    look: Vec3,
) {
    *camera = Transform::from_translation(pos).looking_at(look, Vec3::Y);
}

fn run_tour(
    mut runner: ResMut<Runner>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut camera: Single<&mut Transform, With<ShotCamera>>,
    camera_info: Single<(&Camera, &GlobalTransform, &RenderTarget), With<ShotCamera>>,
    mut pointers: Query<&mut PointerLocation>,
    mut pointer_input: MessageWriter<PointerInput>,
    probe: Res<ProbePointer>,
    buttons: Single<(Entity, &GlobalTransform), With<Button>>,
    window: Single<Entity, With<PrimaryWindow>>,
) {
    runner.frame += 1;
    let step = runner.frame / STEP_FRAMES;
    let sub = runner.frame % STEP_FRAMES;

    let (cam, cam_gt, render_target) = camera_info.into_inner();

    let overview = (Vec3::new(0.0, 0.22, 1.05), Vec3::ZERO);
    let close = (Vec3::new(0.0, 0.0, 0.24), Vec3::ZERO);
    let oblique = (Vec3::new(0.72, 0.40, 0.75), Vec3::new(0.0, 0.02, 0.0));
    let far = (Vec3::new(0.0, 0.0, 2.4), Vec3::ZERO);
    let medium = (Vec3::new(0.0, 0.02, 0.62), Vec3::ZERO);
    // 标题文字中心（面板 A：content 左上 + 标题半宽/半高）
    let text_target = Vec3::new(-0.14, 0.144, 0.0);
    let text_zoom = (text_target + Vec3::new(0.0, 0.0, 0.24), text_target);

    let mut shoot = |runner: &mut Runner, commands: &mut Commands, name: &str| {
        let path = format!("{}/{}.png", runner.shot_dir, name);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        runner.shots_taken.push(path);
        info!("[spike] screenshot -> {name}");
    };

    match step {
        0..=4 => set_camera(&mut camera, overview.0, overview.1),
        5 if sub == 0 => shoot(&mut runner, &mut commands, "01_overview"),
        6 => set_camera(&mut camera, close.0, close.1),
        7 if sub == 0 => shoot(&mut runner, &mut commands, "02_close"),
        8 => set_camera(&mut camera, oblique.0, oblique.1),
        9 if sub == 0 => shoot(&mut runner, &mut commands, "03_oblique"),
        10 => set_camera(&mut camera, far.0, far.1),
        11 if sub == 0 => shoot(&mut runner, &mut commands, "04_far"),
        12 => set_camera(&mut camera, medium.0, medium.1),
        13 if sub == 0 => shoot(&mut runner, &mut commands, "05_medium"),
        14 => {
            set_camera(&mut camera, overview.0, overview.1);
            // Hover: project the button's world centre into the window and park the synthetic
            // pointer there. The real picking pipeline handles the rest.
            let (button_entity, button_gt) = *buttons;
            let world = button_gt.translation();
            match cam.world_to_viewport(cam_gt, world) {
                Ok(screen) => {
                    let target = NormalizedRenderTarget::Window(
                        WindowRef::Entity(*window)
                            .normalize(Some(*window))
                            .expect("primary window normalizes"),
                    );
                    if let Ok(mut loc) = pointers.get_mut(probe.0) {
                        loc.location = Some(Location { target, position: screen });
                    }
                    if sub == 0 {
                        info!(
                            "[spike] probe: button {button_entity:?} world={world:?} screen={screen:?} \
                             target_matches_primary={}",
                            render_target.normalize(Some(*window)).is_some()
                        );
                    }
                }
                Err(e) => {
                    if sub == 0 {
                        warn!("[spike] probe: world_to_viewport failed: {e:?}");
                    }
                }
            }
        }
        15 if sub == 0 => {
            let (_, button_gt) = *buttons;
            if let Ok(screen) = cam.world_to_viewport(cam_gt, button_gt.translation()) {
                let target = NormalizedRenderTarget::Window(
                    WindowRef::Entity(*window)
                        .normalize(Some(*window))
                        .expect("primary window normalizes"),
                );
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target, position: screen },
                    PointerAction::Press(PointerButton::Primary),
                ));
                info!("[spike] probe: synthetic PRESS at {screen:?}");
            }
        }
        16 if sub == 0 => {
            let (_, button_gt) = *buttons;
            if let Ok(screen) = cam.world_to_viewport(cam_gt, button_gt.translation()) {
                let target = NormalizedRenderTarget::Window(
                    WindowRef::Entity(*window)
                        .normalize(Some(*window))
                        .expect("primary window normalizes"),
                );
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target, position: screen },
                    PointerAction::Release(PointerButton::Primary),
                ));
                info!("[spike] probe: synthetic RELEASE at {screen:?}");
            }
        }
        18 if sub == 0 => {
            info!("[spike] tour done: shots={:?}", runner.shots_taken);
        }
        19 => set_camera(&mut camera, text_zoom.0, text_zoom.1),
        20 if sub == 0 => shoot(&mut runner, &mut commands, "06_text_zoom"),
        22 if sub == 0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
