//! `occlusion_probe`: minimal reproduction rig for the "opaque cube behind the panel
//! shows up in front of it" report.
//!
//! Scene is the same minimal one as `rig-feathers/src/main.rs`:
//! * `Camera3d` at (0, 0.03, 1.45) looking at the origin,
//! * an **opaque** `StandardMaterial` cube at (0.68, -0.18, -0.28) (behind the panel plane z=0),
//! * one `WorldPanel` + `MirrorRoot` UI subtree (780 x `PROBE_PANEL_H` px, `Overflow::scroll_y()`).
//!
//! Controls (env):
//! * `PROBE_PANEL_H` — panel height in px (default 380; report used 300)
//! * `PROBE_SCROLL`  — `ScrollPosition.y` applied at frame 25 (default 0; report used 40)
//! * `PROBE_SHOT_DIR`— screenshot directory (default `../render/probe`)
//! * `PROBE_CUBE_Z`  — cube z (default -0.28)
//! * `PROBE_FRONT_CUBE` — if set, also spawn an opaque cube *in front* of the panel (z=+0.20)
//!   as a depth-test control.
//!
//! Screenshots: `a_before.png` (frame 20, scroll 0), `b_after.png` (frame 45, after scroll).
//! Both from the same camera, same everything else, so only the panel change differs.

use std::collections::HashSet;
use std::path::PathBuf;

use bevy::core_pipeline::core_3d::Transparent3d;
use bevy::pbr::{Material, MeshMaterial3d};
use bevy::prelude::*;
use bevy::render::render_phase::{PhaseItem, ViewSortedRenderPhases};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::text::{FontSize, LineBreak, TextLayout};
use bevy::ui::{BorderRadius, Overflow, ScrollPosition, UiRect, percent, px};
use bevy_luma::engine::layout::render::material_3d::UNodeMaterial3d;

use feathers_in_3d::{FeathersIn3dPlugin, MirrorRoot, PanelNode, PanelSort, WorldPanel};

const PANEL_W: f32 = 780.0;
const UNITS_TO_WORLD: f32 = 0.0018;
const ROW_H: f32 = 18.0;
const FONT_PX: f32 = 13.0;

#[derive(Resource, Clone)]
struct Cfg {
    panel_h: f32,
    scroll: f32,
    cube_z: f32,
    front_cube: bool,
    tour_cam: bool,
    clobber_bg: bool,
    shot_dir: PathBuf,
}

#[derive(Resource, Default)]
struct ProbeFrame(u32);

#[derive(Component)]
struct ProbeCube;

#[derive(Component)]
struct ProbePanel;

#[derive(Component)]
struct ShotCamera;

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let shot_dir = std::env::var("PROBE_SHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("../render/probe"));
    std::fs::create_dir_all(&shot_dir).ok();
    let shot_dir = std::fs::canonicalize(&shot_dir).unwrap_or(shot_dir);

    let cfg = Cfg {
        panel_h: env_f32("PROBE_PANEL_H", 380.0),
        scroll: env_f32("PROBE_SCROLL", 0.0),
        cube_z: env_f32("PROBE_CUBE_Z", -0.28),
        front_cube: std::env::var("PROBE_FRONT_CUBE").is_ok(),
        tour_cam: std::env::var("PROBE_TOUR_CAM").is_ok(),
        clobber_bg: std::env::var("PROBE_CLOBBER_BG").is_ok(),
        shot_dir,
    };
    info!(
        "[probe] panel_h={} scroll={} cube_z={} front_cube={} shot_dir={:?}",
        cfg.panel_h, cfg.scroll, cfg.cube_z, cfg.front_cube, cfg.shot_dir
    );

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "occlusion_probe".into(),
            resolution: (1280, 800).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FeathersIn3dPlugin)
    .insert_resource(cfg)
    .init_resource::<ProbeFrame>()
    .add_systems(Startup, setup)
    .add_systems(Update, (probe_seq, probe_screens, watch_panel_bg))
    .add_systems(Last, probe_dump);
    install_render_probe(&mut app);
    app.run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    cfg: Res<Cfg>,
) {
    commands.spawn((
        ShotCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 45_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.03, 1.45).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 2200.0,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let cube_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.45, 0.15),
        perceptual_roughness: 0.6,
        ..default()
    });
    let cube = commands
        .spawn((
            ProbeCube,
            Mesh3d(meshes.add(Cuboid::new(0.16, 0.16, 0.16))),
            MeshMaterial3d(cube_mat.clone()),
            Transform::from_xyz(0.68, -0.18, cfg.cube_z),
        ))
        .id();
    let front_cube = cfg.front_cube.then(|| {
        commands
            .spawn((
                // deliberately not `ProbeCube`: it must not be mistaken for the reference cube
                Mesh3d(meshes.add(Cuboid::new(0.12, 0.12, 0.12))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.2, 0.8, 0.4),
                    perceptual_roughness: 0.6,
                    ..default()
                })),
                Transform::from_xyz(-0.2, 0.12, 0.20),
            ))
            .id()
    });

    let panel = commands
        .spawn((
            ProbePanel,
            Node {
                position_type: PositionType::Absolute,
                // laid out off-screen, exactly like rig-feathers
                left: px(-20000.0),
                top: px(0.0),
                width: px(PANEL_W),
                height: px(cfg.panel_h),
                padding: UiRect::all(px(16.0)),
                border: UiRect::all(px(2.0)),
                border_radius: BorderRadius::all(px(14.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.045, 0.055, 0.08)),
            BorderColor::all(Color::srgb(0.25, 0.62, 0.95)),
            ScrollPosition::default(),
            WorldPanel {
                units_to_world: UNITS_TO_WORLD,
                anchor: Transform::default(),
            },
            MirrorRoot,
        ))
        .with_children(|panel| {
            const LINES: [&str; 15] = [
                "occlusion probe | panel height from PROBE_PANEL_H",
                "",
                "> find a pair of running shoes under 500",
                "  latin-only text, default built-in font",
                "",
                "* search_products  query=\"running shoe\"  max=500",
                "  #  name                          price  shop",
                "  1  Nike Air Zoom Pegasus 40     469    official",
                "  2  ASICS Gel-Kayano 30          499    runner",
                "  3  Li-Ning ChiTu 7 Pro          439    li-ning",
                "",
                "  Recommend #1: best cushioning for the price.",
                "  Add it to the cart?",
                "",
                "model=mock  tokens=312  ctx=12%",
            ];
            for (i, line) in LINES.iter().enumerate() {
                let mut row = panel.spawn(Node {
                    width: percent(100.0),
                    height: px(ROW_H),
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(px(3.0)),
                    ..default()
                });
                if i % 4 == 1 {
                    row.insert(BackgroundColor(Color::srgba(0.10, 0.14, 0.22, 0.55)));
                }
                row.with_children(|row| {
                    row.spawn((
                        Text::new(*line),
                        TextFont {
                            font_size: FontSize::Px(FONT_PX),
                            ..default()
                        },
                        TextColor(Color::srgb(0.80, 0.85, 0.90)),
                        TextLayout {
                            linebreak: LineBreak::NoWrap,
                            ..default()
                        },
                    ));
                });
            }
        })
        .id();

    info!(
        "[probe] cube entity={:?} (index {}) front_cube={:?} panel entity={:?} (index {})",
        cube,
        cube.index().index(),
        front_cube,
        panel,
        panel.index().index()
    );
}

fn probe_seq(
    mut frame: ResMut<ProbeFrame>,
    mut commands: Commands,
    cfg: Res<Cfg>,
    mut exit: MessageWriter<AppExit>,
    mut scrolls: Query<&mut ScrollPosition, With<MirrorRoot>>,
    mut cameras: Query<&mut Transform, With<ShotCamera>>,
    mut panel_bg: Query<&mut BackgroundColor, With<MirrorRoot>>,
) {
    frame.0 += 1;
    let f = frame.0;

    // Optional replay of rig-feathers' camera tour + scroll (the 06:42 tour that
    // loses the panel body at the 04_text_zoom step).
    if cfg.tour_cam {
        let set = |cameras: &mut Query<&mut Transform, With<ShotCamera>>, pos: Vec3, look: Vec3| {
            for mut camera in cameras.iter_mut() {
                *camera = Transform::from_translation(pos).looking_at(look, Vec3::Y);
            }
            info!("[probe] camera -> {pos:?} look {look:?}");
        };
        match f {
            20 => set(&mut cameras, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO),
            38 => shoot(&mut commands, &cfg, "t01_overview"),
            56 => set(&mut cameras, Vec3::new(0.0, 0.0, 0.62), Vec3::ZERO),
            74 => shoot(&mut commands, &cfg, "t02_near"),
            92 => set(&mut cameras, Vec3::new(0.22, 0.10, 1.35), Vec3::ZERO),
            110 => shoot(&mut commands, &cfg, "t03a_mild_oblique"),
            128 => set(&mut cameras, Vec3::new(0.62, 0.42, 0.86), Vec3::ZERO),
            146 => shoot(&mut commands, &cfg, "t03b_oblique45"),
            164 => set(
                &mut cameras,
                Vec3::new(-0.30, 0.26, 0.40),
                Vec3::new(-0.30, 0.26, 0.0),
            ),
            182 => shoot(&mut commands, &cfg, "t04_text_zoom"),
            200 => {
                set(&mut cameras, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO);
                for mut scroll in &mut scrolls {
                    scroll.0.y = cfg.scroll;
                    info!("[probe] set ScrollPosition.y={} (frame {f})", cfg.scroll);
                }
            }
            218 => shoot(&mut commands, &cfg, "t05_scrolled"),
            240 => {
                info!("[probe] tour replay done");
                exit.write(AppExit::Success);
            }
            _ => {}
        }
        return;
    }

    match f {
        50 => {
            for mut scroll in &mut scrolls {
                scroll.0.y = cfg.scroll;
                info!("[probe] set ScrollPosition.y={} (frame {f})", cfg.scroll);
            }
        }
        30 => shoot(&mut commands, &cfg, "warmup"),
        70 => shoot(&mut commands, &cfg, "a_before"),
        90 if cfg.clobber_bg => {
            // Simulates rig-feathers' `unhover_row` running on the panel root: it is an
            // observer for `PointerOut`, which auto-propagates from a row to its ancestors,
            // and writes the transparent band colour into whatever entity it is called for.
            for mut bg in &mut panel_bg {
                bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.0);
                info!("[probe] SIMULATED bubbled PointerOut: panel BackgroundColor -> alpha 0");
            }
        }
        120 => shoot(&mut commands, &cfg, "b_after"),
        160 => {
            info!("[probe] done; exiting");
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// Logs every change of the panel's `BackgroundColor` (the root of the artifact could be
/// a color write, not a clip/depth issue).
fn watch_panel_bg(
    mut changed: Query<(&BackgroundColor, Entity), (With<MirrorRoot>, Changed<BackgroundColor>)>,
    mut last: Local<Option<Color>>,
) {
    for (bg, entity) in changed.iter_mut() {
        info!("[probe] panel BackgroundColor on {entity:?} -> {:?}", bg.0);
        *last = Some(bg.0);
    }
}

fn probe_screens(
    frame: Res<ProbeFrame>,
    shots: Query<Entity, With<Screenshot>>,
    capturing: Query<Entity, With<bevy::render::view::window::screenshot::Capturing>>,
    captured: Query<Entity, With<bevy::render::view::window::screenshot::Captured>>,
) {
    let f = frame.0;
    if matches!(f, 68..=72 | 118..=124) {
        info!(
            "[probe] screenshots f={f} pending={} capturing={} captured={}",
            shots.iter().count(),
            capturing.iter().count(),
            captured.iter().count()
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_dump(
    frame: Res<ProbeFrame>,
    cfg: Res<Cfg>,
    panels: Query<(Entity, &ComputedNode, &UiGlobalTransform, &ScrollPosition), With<MirrorRoot>>,
    panel_nodes: Query<
        (Entity, &PanelNode, &Transform, &GlobalTransform, Option<&PanelSort>),
        With<PanelNode>,
    >,
    node_mats: Res<Assets<UNodeMaterial3d>>,
    std_mats: Res<Assets<StandardMaterial>>,
    cubes: Query<(Entity, &MeshMaterial3d<StandardMaterial>, &Transform, &GlobalTransform), With<ProbeCube>>,
) {
    match frame.0 {
        72 => dump_state(&cfg, "before-clobber", &panels, &panel_nodes, &node_mats, &std_mats, &cubes),
        122 => dump_state(&cfg, "after-clobber", &panels, &panel_nodes, &node_mats, &std_mats, &cubes),
        _ => {}
    }
}

fn shoot(commands: &mut Commands, cfg: &Cfg, name: &str) {
    let path = cfg.shot_dir.join(format!("{name}.png"));
    let path_str = path.to_string_lossy().to_string();
    info!("[probe] screenshot -> {path_str}");
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path_str));
}

fn dump_state(
    cfg: &Cfg,
    tag: &str,
    panels: &Query<(Entity, &ComputedNode, &UiGlobalTransform, &ScrollPosition), With<MirrorRoot>>,
    panel_nodes: &Query<
        (Entity, &PanelNode, &Transform, &GlobalTransform, Option<&PanelSort>),
        With<PanelNode>,
    >,
    node_mats: &Assets<UNodeMaterial3d>,
    std_mats: &Assets<StandardMaterial>,
    cubes: &Query<(Entity, &MeshMaterial3d<StandardMaterial>, &Transform, &GlobalTransform), With<ProbeCube>>,
) {
    info!(
        "[probe][{tag}] panel_h={} wanted_scroll={}",
        cfg.panel_h, cfg.scroll
    );
    for (entity, cn, ugt, scroll) in panels.iter() {
        info!(
            "[probe][{tag}] panel {:?} size={:?} content_size={:?} scroll={:?} layout_centre={:?}",
            entity,
            cn.size,
            cn.content_size,
            scroll.0,
            ugt.affine().translation
        );
    }
    for (entity, node, transform, gt, sort) in panel_nodes.iter() {
        let material = node_mats.get(&node.material);
        match material {
            Some(mat) => info!(
                "[probe][{tag}] node {:?} depth={} cached_size={:?} local={:?} world={:?} sort_bias={:?} clip_half={:?} clip_inv_x0={:?} color={:?} alpha_mode={:?}",
                entity,
                node.depth,
                node.cached_size,
                transform.translation,
                gt.translation(),
                sort.map(|s| s.bias),
                mat.clip_bounds[0],
                mat.clip_inv_x[0],
                mat.color,
                mat.alpha_mode(),
            ),
            None => info!(
                "[probe][{tag}] node {:?} depth={} cached_size={:?} local={:?} world={:?} (material gone)",
                entity,
                node.depth,
                node.cached_size,
                transform.translation,
                gt.translation(),
            ),
        }
    }
    for (entity, material, transform, gt) in cubes.iter() {
        if let Some(mat) = std_mats.get(&material.0) {
            info!(
                "[probe][{tag}] cube {:?} alpha_mode={:?} local={:?} world={:?}",
                entity,
                mat.alpha_mode,
                transform.translation,
                gt.translation()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Render-world instrumentation: which items reach Transparent3d, in what order,
// and do any of them not belong to a panel?
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
struct ProbeIds {
    panel: HashSet<u32>,
    cube: HashSet<u32>,
}

#[derive(Resource, Default)]
struct RenderFrame(u32);

fn extract_probe_ids(
    mut ids: ResMut<ProbeIds>,
    mut render_frame: ResMut<RenderFrame>,
    sorted: Extract<Query<Entity, With<PanelSort>>>,
    cubes: Extract<Query<Entity, With<ProbeCube>>>,
) {
    ids.panel.clear();
    ids.cube.clear();
    for entity in sorted.iter() {
        ids.panel.insert(entity.index().index());
    }
    for entity in cubes.iter() {
        ids.cube.insert(entity.index().index());
    }
    render_frame.0 = render_frame.0.wrapping_add(1);
}

fn dump_transparent_phase(
    ids: Res<ProbeIds>,
    render_frame: Res<RenderFrame>,
    phases: Res<ViewSortedRenderPhases<Transparent3d>>,
) {
    let f = render_frame.0;
    // a few dumps, 1-based-ish because the extract schedule also ticks once at startup
    if !matches!(f, 22 | 24 | 44 | 46 | 60) {
        return;
    }
    for (view, phase) in phases.0.iter() {
        info!(
            "[probe][render f={f}] transparent view={:?} items={}",
            view,
            phase.items.len()
        );
        for item in phase.items.values() {
            let main = item.main_entity();
            let index = main.id().index().index();
            info!(
                "[probe][render f={f}]   item main={:?} panel_item={} cube_item={} distance={:.6}",
                main,
                ids.panel.contains(&index),
                ids.cube.contains(&index),
                item.distance
            );
        }
    }
    info!(
        "[probe][render f={f}] panel entities with PanelSort={} reference cubes={}",
        ids.panel.len(),
        ids.cube.len()
    );
}

fn install_render_probe(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<ProbeIds>()
        .init_resource::<RenderFrame>()
        .add_systems(ExtractSchedule, extract_probe_ids)
        .add_systems(
            Render,
            dump_transparent_phase
                .in_set(RenderSystems::PhaseSort)
                .after(bevy::render::render_phase::sort_phase_system::<Transparent3d>),
        );
}
