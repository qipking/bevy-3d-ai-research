//! rig_harness_in_3d 的示例应用：rig-ecs 内核 + rig_niri 视图 + feathers_in_3d 渲染。
//!
//! 这个 binary 是**通用 app 模板的样板**：一个 echo 工具 + 一个需要确认的写工具，
//! 加上自动巡游截图。换业务时只改 `kernel.rs`（工具 + 闸门）与这里的少量装配。
//!
//! This binary is the adapter proof: mock `LookLine` rows (the same rows rig-tui
//! draws in a terminal) become feathers `Node`/`Text`/`TextSpan` nodes, which the
//! route-B mirror turns into world-space meshes and the glyph bridge turns into
//! real 3D text.

// 这个 binary 是**示例业务**：只放工具/闸门/巡游。
// 前端能力全部来自 `rig_niri`（lib）。

mod kernel;

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::camera::NormalizedRenderTarget;
use bevy::picking::pointer::{
    Location, PointerAction, PointerButton, PointerId, PointerInput, PointerLocation,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::window::{PrimaryWindow, WindowRef};

use crate::kernel::KernelPlugin;
use rig_niri::batch::{BatchPlugin, CompletedCalls};
use rig_niri::camera_rig::RigCamera;
use rig_niri::frame::{PanelAreas, PanelRoots};
use rig_niri::pane::{PaneScroll, TranscriptRows};
use rig_harness::pick::{PickItem, PickPreview};
use rig_harness::prelude::{Highlighted, Picker};
use rig_niri::panels::PanelId;
use rig_niri::theme::Theme;
use rig_niri::transcript::{RowText, TranscriptFont};
use feathers_in_3d::{MirrorRoot, WorldPanel};

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

    let mut app = rig_harness_in_3d::app("rig_niri: rig-tui looks in feathers + 3D");
    app.add_plugins(KernelPlugin)
        .add_plugins(BatchPlugin)
        .insert_resource(Runner {
            frame: 0,
            dir,
            shots: Vec::new(),
        })
        .init_resource::<Selected>()
        .init_resource::<CompletedCalls>()
        .init_resource::<Theme>()
        .add_systems(Startup, (setup, spawn_probe_pointer))
        .add_systems(
            Update,
            (
                run_tour,
                spawn_row_markers,
                scroll_panel,
                copy_selection,
                demo_preview,
            ),
        )
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

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        ShotCamera,
        RigCamera,
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

    // Latin 用 feathers 的等宽字体，中文回退到 Noto Sans CJK SC（tools/fetch_cjk_font.sh 生成）
    let mono: Handle<Font> = asset_server.load(bevy_feathers::constants::fonts::MONO);
    let cjk: Handle<Font> = asset_server.load("fonts/NotoSansCJKsc-Subset.otf");
    let font: FontSource = FontSource::List(vec![mono.into(), cjk.into()]);
    commands.insert_resource(TranscriptFont(font));
}

/// 演示插件侧的 preview：picker 的 `Highlighted` 变了就换 `PickPreview::Text`
/// （rig-tui 里这是插件自己的事；我们这里用一个假插件把效果显出来）。
fn demo_preview(
    pickers: Query<(Entity, &Picker, Option<&Highlighted>)>,
    mut commands: Commands,
) {
    for (entity, picker, highlighted) in pickers.iter() {
        // 只给多选的（树）picker 挂 preview：单选那块留着看"没有 preview 时列表占满"。
        if !picker.multiple {
            continue;
        }
        let Some(Highlighted(index)) = highlighted else {
            continue;
        };
        let Some(item) = picker.items.get(*index) else {
            continue;
        };
        let text = format!(
            "{}\n\n{}\n\n命令：{}\n序号：{}",
            item.label,
            item.detail.as_deref().unwrap_or("（没有说明）"),
            match item.command.is_empty() {
                true => "（不执行命令）",
                false => item.command.as_str(),
            },
            index,
        );
        commands.entity(entity).insert(PickPreview::Text(text));
    }
}

/// 滚轮 -> 转写窗格的滚动（按行，和 rig-tui 的 pane 一样是行滚动不是像素滚动）。
fn scroll_panel(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut scroll: ResMut<PaneScroll>,
    areas: Res<PanelAreas>,
    roots: Res<PanelRoots>,
    rows: Res<TranscriptRows>,
) {
    let mut delta = 0.0;
    for event in wheel.read() {
        delta += match event.unit {
            bevy::input::mouse::MouseScrollUnit::Line => event.y,
            bevy::input::mouse::MouseScrollUnit::Pixel => event.y / 18.0,
        };
    }
    let steps = delta.round() as i64;
    if steps == 0 {
        return;
    }
    let height = roots
        .entity(PanelId::Transcript)
        .and_then(|root| areas.get(root))
        .map_or(0, |area| usize::from(area.rows));
    if height == 0 {
        return;
    }
    let max_top = rows.len().saturating_sub(height);
    let rows_moved = usize::try_from(steps.unsigned_abs()).unwrap_or(usize::MAX) * 3;
    if steps > 0 {
        scroll.up(rows_moved);
    } else {
        scroll.down(rows_moved, max_top);
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
        debug!("click hit {:?} (not a row)", click.entity);
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
    // `PointerOut` **冒泡到所有祖先**（bevy_picking/src/events.rs）。没有这个 guard 时，
    // 行一离开 hover，冒泡到面板根的 PointerOut 会把面板背景写成 alpha=0 →
    // UNodeMaterial3d 的 SDF body 全部 discard，背后的场景几何就会从"洞"里露出来。
    if !rows.contains(entity) || selected.0.contains(&entity) {
        return;
    }
    if let Ok(mut bg) = backgrounds.get_mut(entity) {
        bg.0 = Color::srgba(0.10, 0.14, 0.22, 0.0);
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
        Ok(()) => info!("[rig_niri] copied {} bytes: {}", text.len(), text.lines().next().unwrap_or("")),
        Err(error) => warn!("[rig_niri] clipboard failed: {error:?}"),
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

/// 模拟敲一个字符（picker 的过滤用；`text` 才有内容）。
fn press_text(
    keys: &mut MessageWriter<KeyboardInput>,
    window: Entity,
    code: KeyCode,
    char: char,
) {
    keys.write(KeyboardInput {
        key_code: code,
        logical_key: Key::Character(char.to_string().into()),
        state: ButtonState::Pressed,
        repeat: false,
        text: Some(char.to_string().into()),
        window,
    });
}

/// 模拟一次按键（巡游里点 picker 用）。
fn press_key(keys: &mut MessageWriter<KeyboardInput>, window: Entity, code: KeyCode, key: Key) {
    keys.write(KeyboardInput {
        key_code: code,
        logical_key: key,
        state: ButtonState::Pressed,
        repeat: false,
        text: None,
        window,
    });
}

fn set_cam(camera: &mut Query<&mut Transform, With<ShotCamera>>, pos: Vec3, look: Vec3) {
    // RF_RIG=1 时相机交给操作架（手柄/键盘），巡游不再抢机位
    if std::env::var("RF_RIG").is_ok() {
        return;
    }
    // Niri 模式：相机由 niri_camera 定在列带正前方，巡游不抢。
    if std::env::var("RF_NIRI_LAYOUT").is_ok() {
        return;
    }
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
    mut scrolls: Query<&mut PaneScroll>,
    mut keyboard: MessageWriter<KeyboardInput>,
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
        7 => set_cam(&mut camera, Vec3::new(0.0, 0.02, 1.08), Vec3::ZERO),
        8 if sub == 0 => shoot(&mut runner, &mut commands, "02_near"),
        9 => set_cam(&mut camera, Vec3::new(0.22, 0.10, 1.35), Vec3::ZERO),
        10 if sub == 0 => shoot(&mut runner, &mut commands, "03a_mild_oblique"),
        11 => set_cam(&mut camera, Vec3::new(0.62, 0.42, 0.86), Vec3::ZERO),
        12 if sub == 0 => shoot(&mut runner, &mut commands, "03b_oblique45"),
        13 => {
            let target = Vec3::new(-0.42, 0.33, 0.0);
            set_cam(&mut camera, target + Vec3::new(0.0, 0.0, 0.55), target);
        }
        14 if sub == 0 => shoot(&mut runner, &mut commands, "04_text_zoom"),
        15 => {
            set_cam(&mut camera, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO);
            if let Ok(mut scroll) = scrolls.single_mut() {
                scroll.follow = false;
                scroll.top = scroll.top.saturating_sub(4);
            }
        }
        16 if sub == 0 => shoot(&mut runner, &mut commands, "05_scrolled"),
        17 => {
            if let Ok(mut scroll) = scrolls.single_mut() {
                scroll.follow = true;
            }
        }
        // 闸门：此时 create_order 被按住，Ask 已经画在面板里
        18 if sub == 0 => shoot(&mut runner, &mut commands, "06_gate_ask"),
        // 选中一行商品并复制
        19 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "Nike Air Zoom") {
                move_pointer(&probe, &mut pointers, &window_entity, screen, &mut pointer_input);
            }
        }
        20 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "Nike Air Zoom") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Press(PointerButton::Primary),
                ));
            }
        }
        21 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "Nike Air Zoom") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Release(PointerButton::Primary),
                ));
                info!("[demo] selected a product row");
            }
        }
        22 if sub == 0 => {
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
        // 点"确认支付"：闸门放行，订单真正执行
        23 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "确认支付") {
                move_pointer(&probe, &mut pointers, &window_entity, screen, &mut pointer_input);
            }
        }
        24 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "确认支付") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Press(PointerButton::Primary),
                ));
                info!("[demo] click confirm");
            }
        }
        25 if sub == 0 => {
            if let Some(screen) = row_screen_pos(&camera_info, &rows, "确认支付") {
                pointer_input.write(PointerInput::new(
                    PointerId::Touch(7),
                    Location { target: normalized_target(&window_entity), position: screen },
                    PointerAction::Release(PointerButton::Primary),
                ));
            }
        }
        27 if sub == 0 => shoot(&mut runner, &mut commands, "07_gate_confirmed"),
        // picker：条带最右边的一列（rig-harness 的 `Picker`）
        30 if sub == 0 => {
            commands.spawn(
                Picker::new("选择模型")
                    .body("↑↓ 选一个，回车确认；Esc 关闭。打字过滤列表。")
                    .item("openai/gpt-5", "")
                    .item("anthropic/claude-sonnet-4-5", "")
                    .item("deepseek/deepseek-chat", "")
                    .item("google/gemini-2.5-pro", "")
                    .item("本地 · llama.cpp", "")
                    .item("（不选，关闭）", ""),
            );
        }
        31 if sub == 0 => shoot(&mut runner, &mut commands, "09_picker"),
        32 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::ArrowDown, Key::ArrowDown);
            press_key(&mut keyboard, window_entity, KeyCode::ArrowDown, Key::ArrowDown);
        }
        33 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::ArrowDown, Key::ArrowDown);
        }
        34 if sub == 0 => shoot(&mut runner, &mut commands, "10_picker_selected"),
        // 斜着看：浮层在 3D 里也是盖在转写上面的（层排序，不靠距离）
        35 => set_cam(&mut camera, Vec3::new(0.62, 0.42, 0.86), Vec3::ZERO),
        36 if sub == 0 => shoot(&mut runner, &mut commands, "10b_picker_oblique"),
        37 => set_cam(&mut camera, Vec3::new(0.0, 0.03, 1.45), Vec3::ZERO),
        // 打字过滤：四个字符后列表只剩 deepseek
        38 if sub == 0 => {
            for (code, char) in [
                (KeyCode::KeyD, 'd'),
                (KeyCode::KeyE, 'e'),
                (KeyCode::KeyE, 'e'),
                (KeyCode::KeyP, 'p'),
            ] {
                press_text(
                    &mut keyboard,
                    window_entity,
                    code,
                    char,
                );
            }
            info!("[demo] picker: typed \"deep\"");
        }
        39 if sub == 0 => shoot(&mut runner, &mut commands, "10c_picker_filtered"),
        40 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::Enter, Key::Enter);
            info!("[demo] picker: enter");
        }
        41 if sub == 0 => shoot(&mut runner, &mut commands, "11_picker_closed"),
        // 第二块 picker：树 + 多选 + preview 列（插件给它挂 PickPreview::Text）
        45 if sub == 0 => {
            let mut picker = Picker::new("会话")
                .body("←/→ 折叠 · Tab 打勾 · 回车提交；右边是插件的 preview。")
                .multiple();
            picker.items.push(PickItem::new("sessions", "")); // 0：分支
            let mut today = PickItem::new("today", "");
            today.parent = Some(0);
            today.detail = Some("12 条".to_string());
            picker.items.push(today); // 1
            let mut yesterday = PickItem::new("yesterday", "");
            yesterday.parent = Some(0);
            yesterday.detail = Some("8 条".to_string());
            picker.items.push(yesterday); // 2
            let mut model = PickItem::new("model", "");
            model.detail = Some("没有树".to_string());
            picker.items.push(model); // 3
            commands.spawn(picker);
        }
        46 if sub == 0 => shoot(&mut runner, &mut commands, "12_tree"),
        47 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::ArrowRight, Key::ArrowRight);
            info!("[demo] tree: open");
        }
        48 if sub == 0 => shoot(&mut runner, &mut commands, "12b_tree_open"),
        49 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::ArrowDown, Key::ArrowDown);
            press_key(&mut keyboard, window_entity, KeyCode::ArrowDown, Key::ArrowDown);
            press_key(&mut keyboard, window_entity, KeyCode::Tab, Key::Tab);
            info!("[demo] tree: select yesterday + tick");
        }
        50 if sub == 0 => shoot(&mut runner, &mut commands, "12c_tree_ticked"),
        51 if sub == 0 => {
            press_key(&mut keyboard, window_entity, KeyCode::Enter, Key::Enter);
            info!("[demo] tree: enter");
        }
        52 if sub == 0 => shoot(&mut runner, &mut commands, "13_tree_closed"),
        54 if sub == 0 => {
            info!("[rig_niri] tour done: {:?}", runner.shots);
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
