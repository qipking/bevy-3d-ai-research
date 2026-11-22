//! rig-feathers: rig-tui's look output rendered with bevy_feathers inside 3D space.
//!
//! This binary is the adapter proof: mock `LookLine` rows (the same rows rig-tui
//! draws in a terminal) become feathers `Node`/`Text`/`TextSpan` nodes, which the
//! route-B mirror turns into world-space meshes and the glyph bridge turns into
//! real 3D text.

mod style;
mod transcript;

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy_ui::{BorderRadius, Node, UiRect, px};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::transcript::{Row, spawn_rows};
use feathers_in_3d::{FeathersIn3dPlugin, MirrorRoot, WorldPanel};

const PANEL_W: f32 = 780.0;
const PANEL_H: f32 = 440.0;
const UNITS_TO_WORLD: f32 = 0.0018;

#[derive(Component)]
struct ShotCamera;

#[derive(Resource)]
struct Runner {
    frame: u32,
    dir: String,
    shots: Vec<String>,
}

const STEP_FRAMES: u32 = 18;

fn main() {
    let dir = std::env::var("RF_SHOT_DIR").unwrap_or_else(|_| "shots".to_string());
    std::fs::create_dir_all(&dir).ok();

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rig-feathers: rig-tui looks in feathers + 3D".into(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(bevy_feathers::FeathersPlugins)
        .add_plugins(FeathersIn3dPlugin)
        .insert_resource(Runner { frame: 0, dir, shots: Vec::new() })
        .add_systems(Startup, setup)
        .add_systems(Update, run_tour)
        .run();
}



fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
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
        DirectionalLight { illuminance: 2200.0, ..default() },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 参考立方体：证明面板在真 3D 里（遮挡关系）
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.16, 0.16, 0.16))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.45, 0.15),
            perceptual_roughness: 0.6,
            ..default()
        })),
        Transform::from_xyz(0.68, -0.18, -0.28),
    ));

    let font: Handle<Font> = asset_server.load(bevy_feathers::constants::fonts::MONO);

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(-20000.0),
                top: px(0.0),
                width: px(PANEL_W),
                height: px(PANEL_H),
                padding: UiRect::all(px(16.0)),
                border: UiRect::all(px(2.0)),
                border_radius: BorderRadius::all(px(14.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.045, 0.055, 0.08)),
            BorderColor::all(Color::srgb(0.25, 0.62, 0.95)),
            WorldPanel { units_to_world: UNITS_TO_WORLD, anchor: Transform::default() },
            MirrorRoot,
        ))
        .with_children(|panel| {
            spawn_rows(panel, &mock_rows(), &font);
        });
}

/// 一条 mock 转写：就是 rig-tui 的 look 会吐出来的那种行（带样式 span + 色带）。
fn mock_rows() -> Vec<Row> {
    let dim = Style::default().fg(RColor::DarkGray);
    let dimmer = Style::default().fg(RColor::Rgb(90, 96, 110));
    let accent = Style::default().fg(RColor::Yellow).add_modifier(Modifier::BOLD);
    let price = Style::default().fg(RColor::Green);
    let rate = Style::default().fg(RColor::LightYellow);
    let ask = Style::default().fg(RColor::Cyan);
    let text = Style::default().fg(RColor::Rgb(210, 216, 228));

    vec![
        Row::new(Line::from(Span::styled(
            "rig-feathers | rig-tui looks -> feathers nodes -> 3D panels",
            dim,
        )))
        .band(RColor::Rgb(20, 26, 38)),
        Row::new(Line::from(Span::styled("", Style::default()))),
        Row::new(Line::from(Span::styled("> find me a running shoe under 500", ask))),
        Row::new(Line::from(Span::styled("", Style::default()))),
        Row::new(Line::from(vec![
            Span::styled("* ", Style::default().fg(RColor::Green)),
            Span::styled("search_products", accent),
            Span::styled("  query=\"running shoe\"  max=500  limit=3", dim),
        ]))
        .band(RColor::Rgb(24, 34, 52)),
        Row::new(Line::from(Span::styled(
            "  #  name                          price  shop             rate",
            dimmer,
        ))),
        Row::new(Line::from(vec![
            Span::styled("  1  ", dimmer),
            Span::styled("Nike Air Zoom Pegasus 40     ", text),
            Span::styled("469   ", price),
            Span::styled("official store   ", text),
            Span::styled("4.8", rate),
        ])),
        Row::new(Line::from(vec![
            Span::styled("  2  ", dimmer),
            Span::styled("ASICS Gel-Kayano 30         ", text),
            Span::styled("499   ", price),
            Span::styled("runner shop      ", text),
            Span::styled("4.7", rate),
        ])),
        Row::new(Line::from(vec![
            Span::styled("  3  ", dimmer),
            Span::styled("Li-Ning ChiTu 7 Pro         ", text),
            Span::styled("439   ", price),
            Span::styled("li-ning          ", text),
            Span::styled("4.6", rate),
        ])),
        Row::new(Line::from(Span::styled("", Style::default()))),
        Row::new(Line::from(vec![
            Span::styled("  Recommend ", text),
            Span::styled("#1", Style::default().fg(RColor::LightMagenta).add_modifier(Modifier::BOLD)),
            Span::styled(": the best cushioning for the price.", text),
        ])),
        Row::new(Line::from(vec![
            Span::styled("  Add it to the cart? ", text),
            Span::styled("(this row is a TextSpan chain)", dimmer),
        ])),
        Row::new(Line::from(Span::styled("", Style::default()))),
        Row::new(Line::from(vec![
            Span::styled("model=mock  ", dim),
            Span::styled("tokens=312  ", dim),
            Span::styled("ctx=12%  ", dim),
            Span::styled("| look registry: RenderSystems::Render", dimmer),
        ]))
        .band(RColor::Rgb(18, 22, 32)),
    ]
}

fn set_cam(camera: &mut Query<&mut Transform, With<ShotCamera>>, pos: Vec3, look: Vec3) {
    if let Ok(mut t) = camera.single_mut() {
        *t = Transform::from_translation(pos).looking_at(look, Vec3::Y);
    }
}

fn run_tour(
    mut runner: ResMut<Runner>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut camera: Query<&mut Transform, With<ShotCamera>>,
) {
    runner.frame += 1;
    let step = runner.frame / STEP_FRAMES;
    let sub = runner.frame % STEP_FRAMES;
    let mut shoot = |runner: &mut Runner, commands: &mut Commands, name: &str| {
        let path = format!("{}/{}.png", runner.dir, name);
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
        runner.shots.push(path);
    };

    match step {
        0..=5 => set_cam(&mut camera, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO),
        6 if sub == 0 => shoot(&mut runner, &mut commands, "01_overview"),
        7 => set_cam(&mut camera, Vec3::new(0.0, 0.0, 0.62), Vec3::ZERO),
        8 if sub == 0 => shoot(&mut runner, &mut commands, "02_near"),
        9 => set_cam(&mut camera, Vec3::new(0.22, 0.10, 1.35), Vec3::ZERO),
        10 if sub == 0 => shoot(&mut runner, &mut commands, "03a_mild_oblique"),
        11 => set_cam(&mut camera, Vec3::new(0.62, 0.42, 0.86), Vec3::ZERO),
        12 if sub == 0 => shoot(&mut runner, &mut commands, "03b_oblique45"),
        13 => {
            let target = Vec3::new(-0.30, 0.26, 0.0);
            set_cam(&mut camera, target + Vec3::new(0.0, 0.0, 0.40), target);
        }
        14 if sub == 0 => shoot(&mut runner, &mut commands, "04_text_zoom"),
        16 if sub == 0 => {
            info!("[rig-feathers] tour done: {:?}", runner.shots);
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
