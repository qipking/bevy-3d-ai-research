//! rig-feathers: rig-tui's look output rendered with bevy_feathers inside 3D space.
//!
//! This binary is the adapter proof: mock `LookLine` rows (the same rows rig-tui
//! draws in a terminal) become feathers `Node`/`Text`/`TextSpan` nodes, which the
//! route-B mirror turns into world-space meshes and the glyph bridge turns into
//! real 3D text.

mod batch;
mod style;
mod transcript;

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::camera::NormalizedRenderTarget;
use bevy::picking::pointer::{
    Location, PointerAction, PointerButton, PointerId, PointerInput, PointerLocation,
};
use bevy::window::{PrimaryWindow, WindowRef};
use bevy_ui::{BorderRadius, Node, Overflow, ScrollPosition, UiRect, px};
use rig_tui::ratatui::style::{Color as RColor, Modifier, Style};
use rig_tui::ratatui::text::{Line, Span};

use crate::batch::{BatchPlugin, BatchRows};
use crate::transcript::{Row, RowText, spawn_rows};
use feathers_in_3d::{FeathersIn3dPlugin, MirrorRoot, WorldPanel};

const PANEL_W: f32 = 780.0;
const PANEL_H: f32 = 300.0;
const UNITS_TO_WORLD: f32 = 0.0018;

#[derive(Component)]
struct ShotCamera;

/// 选中的行（按点击顺序不保证，只用于高亮与复制）。
#[derive(Resource, Default)]
struct Selected(std::collections::HashSet<Entity>);

/// 合成指针（Touch 7），让巡游能在没有真实鼠标时悬停/点击 3D 面板。
#[derive(Resource)]
struct ProbePointer(Entity);

#[derive(Component)]
struct SelectedRow;

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
        .add_plugins(BatchPlugin)
        .insert_resource(Runner { frame: 0, dir, shots: Vec::new() })
        .init_resource::<Selected>()
        .init_resource::<BgLogIds>()
        .add_systems(Startup, (setup, spawn_probe_pointer))
        .add_systems(Update, (run_tour, spawn_row_markers, scroll_panel, copy_selection, watch_panel_bg, debug_pickables, log_hits))
        .add_observer(select_row)
        .add_observer(hover_row)
        .add_observer(unhover_row)
        .run();
}



/// 一次性：在"工具行"的左右两端各放一个小方块（纯几何，不经文字），
/// 作为 45° 下判断"文字是否跟色带同一条线"的参照物。
fn spawn_row_markers(
    mut done: Local<bool>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    frame: Res<bevy::diagnostic::FrameCount>,
    panels: Query<(Entity, &WorldPanel), With<MirrorRoot>>,
    nodes: Query<(Entity, &bevy::ui::ComputedNode, &bevy::ui::UiGlobalTransform)>,
    children: Query<&Children>,
) {
    if *done || frame.0 < 30 || std::env::var("RF_MARKERS").is_err() {
        return;
    }
    let Some((panel_entity, panel)) = panels.iter().next() else { return };
    let Ok((_, _, panel_ugt)) = nodes.get(panel_entity) else { return };
    let Ok(kids) = children.get(panel_entity) else { return };
    // 第 5 个子节点 = 工具行（mock_rows 里的第 5 行）
    let Some(row) = kids.iter().nth(4) else { return };
    let Ok((_, cn, ugt)) = nodes.get(row) else { return };
    *done = true;

    let u = panel.units_to_world;
    let off = ugt.affine().translation - panel_ugt.affine().translation;
    let half_w = cn.size.x * 0.5;
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.15, 0.85),
        unlit: true,
        ..default()
    });
    for sign in [-1.0f32, 1.0] {
        commands.spawn((
            Mesh3d(meshes.add(Rectangle::new(0.012, 0.012))),
            MeshMaterial3d(mat.clone()),
            Transform::from_translation(Vec3::new(
                (off.x + sign * half_w) * u,
                -off.y * u,
                0.0,
            )),
        ));
    }
    info!("[markers] tool row {:?} at off=({:.1},{:.1}) half_w={:.1}", row, off.x, off.y, half_w);
}

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

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    batch_rows: Res<BatchRows>,
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

    // Latin 用 feathers 的等宽字体，中文回退到 Noto Sans CJK SC（tools/fetch_cjk_font.sh 生成）
    let mono: Handle<Font> = asset_server.load(bevy_feathers::constants::fonts::MONO);
    let cjk: Handle<Font> = asset_server.load("fonts/NotoSansCJKsc-Regular.otf");
    let font: FontSource = FontSource::List(vec![mono.into(), cjk.into()]);

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
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.045, 0.055, 0.08)),
            BorderColor::all(Color::srgb(0.25, 0.62, 0.95)),
            ScrollPosition::default(),
            WorldPanel { units_to_world: UNITS_TO_WORLD, anchor: Transform::default() },
            MirrorRoot,
        ))
        .with_children(|panel| {
            let mut rows = intro_rows();
            rows.extend(batch_rows.0.iter().cloned());
            rows.push(Row::new(Line::from(Span::styled(
                "  ^ these rows come from rig-tui's RenderBatch + RenderSystems::Render looks",
                Style::default().fg(RColor::Rgb(110, 118, 136)),
            ))));
            spawn_rows(panel, &rows, &font);
        });
}

/// 一条 mock 转写：就是 rig-tui 的 look 会吐出来的那种行（带样式 span + 色带）。
/// 面板顶部的静态行（不属于 RenderBatch），用来对照。
fn intro_rows() -> Vec<Row> {
    let dim = Style::default().fg(RColor::DarkGray);
    let ask = Style::default().fg(RColor::Cyan);
    vec![
        Row::new(Line::from(Span::styled(
            "rig-feathers | rig-tui RenderBatch -> looks -> LookLine -> feathers -> 3D",
            dim,
        )))
        .band(RColor::Rgb(20, 26, 38)),
        Row::new(Line::from(Span::styled("> 帮我找一双 500 以内的跑鞋", ask))),
        Row::new(Line::from(Span::styled("", Style::default()))),
    ]
}

/// 滚轮 -> 面板的 `ScrollPosition`（内容高度取自 `ComputedNode.content_size`）。
fn scroll_panel(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut panels: Query<(&ComputedNode, &mut ScrollPosition), With<MirrorRoot>>,
) {
    let mut delta = 0.0;
    for event in wheel.read() {
        delta += match event.unit {
            bevy::input::mouse::MouseScrollUnit::Line => event.y * 24.0,
            bevy::input::mouse::MouseScrollUnit::Pixel => event.y,
        };
    }
    if delta == 0.0 {
        return;
    }
    for (cn, mut scroll) in panels.iter_mut() {
        let max = (cn.content_size.y - cn.size.y + 32.0).max(0.0);
        scroll.0.y = (scroll.0.y - delta).clamp(0.0, max);
    }
}

/// 点击行：单选 / Shift 扩展 / Ctrl 追加；再点一次取消。
fn select_row(
    click: On<bevy::picking::events::PointerClick>,
    rows: Query<&RowText>,
    parents: Query<&ChildOf>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut selected: ResMut<Selected>,
    mut commands: Commands,
    mut backgrounds: Query<(&mut BackgroundColor, Option<&SelectedRow>)>,
) {
    // 只处理"行"（有 RowText 的实体或其祖先）
    let mut entity = click.entity;
    let row = loop {
        if rows.contains(entity) {
            break Some(entity);
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => break None,
        }
    };
    let Some(row) = row else {
        info!("[bgwatch] click hit {:?} (not a row)", click.entity);
        return;
    };

    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if !shift && !ctrl {
        selected.0.clear();
    }
    if selected.0.contains(&row) && !shift && !ctrl {
        selected.0.clear();
    } else {
        selected.0.insert(row);
    }

    // 重画选择底色
    for (mut bg, was) in backgrounds.iter_mut() {
        if was.is_some() {
            bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.0);
        }
    }
    for &e in selected.0.iter() {
        if let Ok((mut bg, _)) = backgrounds.get_mut(e) {
            bg.0 = Color::srgb(0.16, 0.26, 0.42);
        }
        commands.entity(e).insert(SelectedRow);
    }
}

fn hover_row(
    over: On<bevy::picking::events::PointerOver>,
    rows: Query<&RowText>,
    mut backgrounds: Query<&mut BackgroundColor>,
    selected: Res<Selected>,
) {
    // 只有"行"才提亮；面板背景不是可交互对象
    let entity = over.entity;
    info!("[bgwatch] PointerOver entity={entity:?} is_row={}", rows.contains(entity));
    if !rows.contains(entity) || selected.0.contains(&entity) {
        return;
    }
    if let Ok(mut bg) = backgrounds.get_mut(entity) {
        bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.55);
    }
}

fn unhover_row(
    out: On<bevy::picking::events::PointerOut>,
    rows: Query<&RowText>,
    mut backgrounds: Query<&mut BackgroundColor>,
    selected: Res<Selected>,
) {
    let entity = out.entity;
    info!("[bgwatch] PointerOut entity={entity:?} is_row={}", rows.contains(entity));
    // FIX (env-gated for A/B): `PointerOut` auto-propagates to all ancestors, so this
    // observer also runs for the panel root. Only a real row may lose its band colour.
    let guard = std::env::var("RIG_UNHOVER_GUARD").is_ok();
    if (guard && !rows.contains(entity)) || selected.0.contains(&entity) {
        return;
    }
    if let Ok(mut bg) = backgrounds.get_mut(entity) {
        bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.0);
    }
}

fn log_hits(mut hits: MessageReader<bevy::picking::backend::PointerHits>) {
    for hit in hits.read() {
        let entities: Vec<u32> = hit.picks.iter().map(|(e, _)| e.index().index()).collect();
        if entities.contains(&457) {
            info!(
                "[hits] pointer={:?} order={} has_457=true all={entities:?}",
                hit.pointer, hit.order
            );
        }
    }
}

fn debug_pickables(
    mut frame: Local<u32>,
    panels: Query<(Entity, Has<bevy::picking::Pickable>), With<MirrorRoot>>,
    texts: Query<(Entity, Has<bevy::picking::Pickable>), With<Text>>,
    index: Res<feathers_in_3d::PanelPickIndex>,
) {
    *frame += 1;
    if *frame % 120 != 0 {
        return;
    }
    for (entity, pickable) in panels.iter() {
        info!(
            "[pickdbg] f={} panel={entity:?} has_pickable={pickable} pick_rects={}",
            *frame,
            index.rects.len()
        );
        let listed = index.rects.iter().any(|r| r.entity == entity);
        info!("[pickdbg] panel itself in PanelPickIndex={listed}");
    }
    let text_pickable: Vec<_> = texts.iter().map(|(e, p)| (e.index().index(), p)).collect();
    info!("[pickdbg] text nodes pickable (first 5): {:?}", &text_pickable[..text_pickable.len().min(5)]);
}

#[derive(Resource, Default)]
struct BgLogIds(std::collections::HashSet<u32>);

/// Logs every BackgroundColor change on a mirrored panel node (the panel root included).
fn watch_panel_bg(
    mut changes: Query<(Entity, &BackgroundColor), Changed<BackgroundColor>>,
    panels: Query<Entity, With<MirrorRoot>>,
    mut logged: ResMut<BgLogIds>,
) {
    for (entity, bg) in changes.iter() {
        if panels.contains(entity) || logged.0.insert(entity.index().index()) {
            let panel_node = panels.contains(entity);
            info!(
                "[bgwatch] BackgroundColor entity={entity:?} is_panel_root={panel_node} -> {:?}",
                bg.0
            );
        }
    }
}

/// Ctrl+C：把选中行的文本拼起来写进系统剪贴板。
fn copy_selection(
    keyboard: Res<ButtonInput<KeyCode>>,
    selected: Res<Selected>,
    rows: Query<&RowText>,
    mut clipboard: ResMut<bevy_clipboard::Clipboard>,
) {
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if !(ctrl && keyboard.just_pressed(KeyCode::KeyC)) || selected.0.is_empty() {
        return;
    }
    let mut text = String::new();
    for &e in selected.0.iter() {
        if let Ok(row) = rows.get(e) {
            text.push_str(&row.0);
            text.push('\n');
        }
    }
    match clipboard.set_text(text.clone()) {
        Ok(()) => info!("[rig-feathers] copied {} bytes: {}", text.len(), text.lines().next().unwrap_or("")),
        Err(error) => warn!("[rig-feathers] clipboard failed: {error:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn normalized_target(window: &Entity) -> NormalizedRenderTarget {
    NormalizedRenderTarget::Window(
        WindowRef::Entity(*window)
            .normalize(Some(*window))
            .expect("primary window normalizes"),
    )
}

/// 找到文本含 `needle` 的行，把它在屏幕上的位置算出来。
fn row_screen_pos(
    camera_info: &Single<(&Camera, &GlobalTransform), With<ShotCamera>>,
    rows: &Query<(Entity, &RowText, &GlobalTransform)>,
    needle: &str,
) -> Option<Vec2> {
    let (cam, cam_gt) = &**camera_info;
    let (_, _, gt) = rows.iter().find(|(_, text, _)| text.0.contains(needle))?;
    cam.world_to_viewport(cam_gt, gt.translation()).ok()
}

fn move_pointer(
    probe: &ProbePointer,
    pointers: &mut Query<&mut PointerLocation>,
    window: &Entity,
    screen: Vec2,
    input: &mut MessageWriter<PointerInput>,
) {
    if let Ok(mut location) = pointers.get_mut(probe.0) {
        location.location = Some(Location {
            target: normalized_target(window),
            position: screen,
        });
    }
    input.write(PointerInput::new(
        PointerId::Touch(7),
        Location {
            target: normalized_target(window),
            position: screen,
        },
        PointerAction::Move { delta: Vec2::ZERO },
    ));
}

fn set_cam(camera: &mut Query<&mut Transform, With<ShotCamera>>, pos: Vec3, look: Vec3) {
    if let Ok(mut transform) = camera.single_mut() {
        *transform = Transform::from_translation(pos).looking_at(look, Vec3::Y);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_tour(
    mut runner: ResMut<Runner>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut camera: Query<&mut Transform, With<ShotCamera>>,
    camera_info: Single<(&Camera, &GlobalTransform), With<ShotCamera>>,
    probe: Res<ProbePointer>,
    mut pointers: Query<&mut PointerLocation>,
    mut pointer_input: MessageWriter<PointerInput>,
    window: Single<Entity, With<PrimaryWindow>>,
    rows: Query<(Entity, &RowText, &GlobalTransform)>,
    selected: Res<Selected>,
    mut clipboard: ResMut<bevy_clipboard::Clipboard>,
    mut scrolls: Query<&mut ScrollPosition, With<MirrorRoot>>,
) {
    let window_entity = *window;
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
        // 滚动：ScrollPosition 推到 40 px
        15 => {
            set_cam(&mut camera, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO);
            if let Ok(mut scroll) = scrolls.single_mut() {
                scroll.0.y = 40.0;
            }
        }
        16 if sub == 0 => shoot(&mut runner, &mut commands, "05_scrolled"),
        17 => {
            if let Ok(mut scroll) = scrolls.single_mut() {
                scroll.0.y = 0.0;
            }
        }
        // 交互：合成指针悬停 → 按下 → 抬起 → 复制
        18 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "search_products") {
                move_pointer(&probe, &mut pointers, &window_entity, screen, &mut pointer_input);
                if sub == 0 {
                    info!("[demo] hover tool row at {screen:?}");
                }
            }
        }
        19 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "search_products") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Press(PointerButton::Primary),
                ));
                info!("[demo] click tool row");
            }
        }
        20 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "search_products") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Release(PointerButton::Primary),
                ));
            }
        }
        21 if sub == 0 => {
            let mut text = String::new();
            for &e in selected.0.iter() {
                if let Ok((_, row, _)) = rows.get(e) {
                    text.push_str(&row.0);
                    text.push('\n');
                }
            }
            if !text.is_empty() {
                match clipboard.set_text(text.clone()) {
                    Ok(()) => info!("[demo] copied {} bytes to clipboard", text.len()),
                    Err(e) => warn!("[demo] clipboard failed: {e:?}"),
                }
            } else {
                warn!("[demo] nothing selected");
            }
        }
        22 if sub == 0 => shoot(&mut runner, &mut commands, "06_selected"),
        24 if sub == 0 => {
            info!("[rig-feathers] tour done: {:?}", runner.shots);
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
