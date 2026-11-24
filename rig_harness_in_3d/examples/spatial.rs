//! 斗兽场 + Niri 滚动平铺的原型：**层装 agent，层内用 Niri 管理窗口**。
//!
//! 跑法（在 `rig_harness_in_3d` 目录）：
//!
//! ```sh
//! BEVY_ASSET_ROOT=$PWD RF_SHOT_DIR=../render/rf_niri cargo run --example spatial
//! ```
//!
//! 模型（`docs/design/SPATIAL_UI.md` §16）：每层 = 一个 agent，层的平面在 `z = -R_L`、`y = Y_L`
//! （越外越高越远，像斗兽场台阶）；层里的卡片由 `niri::Tiling` 排版——列宽预设、
//! 列内竖分、`view_offset` 把聚焦列居中。相机在层的高度上平视，不倾斜。
//!
//! 巡游：总览 → 聚焦第 3 列（滚动）→ 切列宽预设 → 进入第 2 层（隐藏第 1 层）。

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::text::FontSource;
use rig_niri::camera_rig::RigCamera;
use rig_niri::niri::{Column, Layer, LayerFocus, PresetSize, Tile, Tiling};
use rig_niri::spatial::{CardContent, CardLook, FreeLook, spawn_card_sized};
use rig_niri::transcript::{Row, TranscriptFont};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

#[derive(Component)]
struct ShotCamera;

#[derive(Resource)]
struct Tour {
    frame: u32,
    dir: String,
}

const STEP: u32 = 20;

fn main() {
    let dir = std::env::var("RF_SHOT_DIR").unwrap_or_else(|_| "shots".to_string());
    std::fs::create_dir_all(&dir).ok();

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "rig spatial prototype: niri tiling on the funnel".to_string(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(bevy_feathers::FeathersPlugins)
        .add_plugins(feathers_in_3d::FeathersIn3dPlugin)
        .add_plugins(rig_niri::niri::TilingPlugin)
        .add_plugins(rig_niri::spatial::SpatialPlugin)
        .insert_resource(Tour { frame: 0, dir })
        .add_systems(Startup, (setup, spawn_rings, spawn_cards).chain())
        .add_systems(Update, tour)
        .run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        ShotCamera,
        RigCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 45_f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(0.0, 0.0, -2.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 2200.0,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(TranscriptFont(FontSource::List(vec![
        asset_server.load(bevy_feathers::constants::fonts::MONO).into(),
        asset_server.load::<Font>("fonts/NotoSansCJKsc-Subset.otf").into(),
    ])));
}

/// 漏斗预览：每层一圈细环（座位线），越外越高越远。
fn spawn_rings(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    tiling: Res<Tiling>,
) {
    for layer in 0..tiling.layers.len() {
        let origin = tiling.layer_origin(layer);
        let radius = tiling.viewport.x * 0.58;
        let material = materials.add(StandardMaterial {
            base_color: Color::srgba(0.35, 0.55, 0.85, 0.30),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        commands.spawn((
            Mesh3d(meshes.add(Torus::new(radius - 0.004, radius + 0.004))),
            MeshMaterial3d(material),
            Transform::from_xyz(origin.x, origin.y, origin.z)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        ));
    }
}

fn line(text: impl Into<String>, style: Style) -> Row {
    Row::new(Line::from(Span::styled(text.into(), style)))
}

fn bold() -> Style {
    Style::default()
        .fg(RColor::LightYellow)
        .add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::default().fg(RColor::Rgb(140, 148, 166))
}

fn body() -> Style {
    Style::default().fg(RColor::Rgb(214, 220, 232))
}

fn look() -> CardLook {
    CardLook {
        background: Color::srgb(0.05, 0.06, 0.085),
        border: Color::srgb(0.30, 0.50, 0.78),
        radius: 10.0,
    }
}

/// 两层（两个 agent），每层三列：转写 / 工具（上下两个）/ 输入。
fn spawn_cards(
    mut commands: Commands,
    mut tiling: ResMut<Tiling>,
    font: Res<TranscriptFont>,
) {
    tiling.layers = vec![
        Layer {
            columns: vec![
                Column::new(PresetSize::Proportion(1.0 / 3.0), 1),
                Column::new(PresetSize::Proportion(1.0 / 2.0), 2),
                Column::new(PresetSize::Proportion(1.0 / 3.0), 1),
            ],
            focus: 0,
            view_offset: 0.0,
            tile: 0,
        },
        Layer {
            columns: vec![
                Column::new(PresetSize::Proportion(0.5), 1),
                Column::new(PresetSize::Proportion(1.0 / 3.0), 1),
            ],
            focus: 0,
            view_offset: 0.0,
            tile: 0,
        },
    ];
    // 总览：短带鱼屏不绕圈，只贴一段弧（半径 = ring_min_radius = 1 m）。
    tiling.ring = Some(tiling.overview_radius());
    info!("[niri] ribbon radius = {:.3} m", tiling.overview_radius());

    // (layer, column, tile, title, rows)
    let cards = [
        (0, 0, 0, "transcript", vec![
            line("assistant", dim()),
            line("推荐第 1 双：Nike", body()),
            line("建议下单前确认", dim()),
        ]),
        (0, 1, 0, "search_products", vec![
            line("(done) query=跑步鞋", dim()),
            line("1 Nike Pegasus 40  469", body()),
            line("2 ASICS Kayano 30  499", body()),
        ]),
        (0, 1, 1, "create_order", vec![
            line("waiting (gate)", bold()),
            line("确认支付 / 取消", dim()),
        ]),
        (0, 2, 0, "composer", vec![
            line("› 还要更轻的", body()),
            line("Enter sends", dim()),
        ]),
        (1, 0, 0, "agent: 研究", vec![
            line("tokens 1.2k", dim()),
            line("3 条来源", body()),
        ]),
        (1, 1, 0, "notes", vec![line("待整理 2 条", body())]),
    ];
    for (layer, column, tile, title, rows) in cards {
        let entity = spawn_card_sized(
            &mut commands,
            &font,
            Vec2::new(0.4, 0.4), // 第一次摆位就会被平铺改掉
            look(),
            CardContent { title, rows },
        );
        commands.entity(entity).insert(Tile {
            layer,
            column,
            tile,
        });
    }
}

fn set_cam(camera: &mut Query<&mut Transform, With<ShotCamera>>, eye: Vec3) {
    if std::env::var("RF_RIG").is_ok() {
        return;
    }
    if let Ok(mut transform) = camera.single_mut() {
        *transform = Transform::from_translation(eye).looking_at(eye + Vec3::NEG_Z, Vec3::Y);
    }
}

fn tour(
    mut tour: ResMut<Tour>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut camera: Query<&mut Transform, With<ShotCamera>>,
    mut tiling: ResMut<Tiling>,
    mut focus: ResMut<LayerFocus>,
    cards: Query<&Tile>,
    mut free: ResMut<FreeLook>,
) {
    tour.frame += 1;
    let step = tour.frame / STEP;
    let sub = tour.frame % STEP;
    let mut shoot = |tour: &mut Tour, commands: &mut Commands, name: &str| {
        let path = format!("{}/{name}.png", tour.dir);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
    };

    match step {
        // 总览（平面）：相机在层的高度上，两层都在。
        // 总览：打开自由相机，然后"按 W 往里走"——证明可以在圆柱内部移动。
        0..=3 => {
            free.0 = true;
            set_cam(&mut camera, Vec3::new(0.0, 0.0, 0.0));
        }
        4 if sub == 0 => shoot(&mut tour, &mut commands, "01_overview"),
        // 在圆柱内部换个机位：斜着看同一条带（自由相机可以停在任何地方）。
        5 => set_cam(&mut camera, Vec3::new(0.45, 0.05, 0.55)),
        6 if sub == 0 => shoot(&mut tour, &mut commands, "02_inside"),
        // 切回单 agent：平面 Niri，隐藏别的层。
        7 if sub == 0 => {
            tiling.ring = None;
            tiling.scale = 1.0;
        }
        8 => set_cam(&mut camera, Vec3::new(0.0, 0.0, 0.0)),
        // 进入第 2 层（agent 研究）：隐藏第 1 层，相机升到第 2 层，
        // 并且换成**环模式**：这一层的列首尾相连地铺在半径 1 m 的圆柱上。
        8 if sub == 0 => shoot(&mut tour, &mut commands, "03_agent0"),
        9 if sub == 0 => {
            focus.active = Some(1);
            tiling.focus_column(1, 1);
            set_cam(&mut camera, Vec3::new(0.0, 0.30, 0.0));
        }
        10 if sub == 0 => shoot(&mut tour, &mut commands, "04_agent1"),
        12 if sub == 0 => {
            let tiles: Vec<(usize, usize, usize)> =
                cards.iter().map(|tile| (tile.layer, tile.column, tile.tile)).collect();
            info!("[niri] tour done, {} cards {tiles:?}", tiles.len());
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
